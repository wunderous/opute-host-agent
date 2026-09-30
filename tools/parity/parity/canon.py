"""Canonicalization, typed masks, and structural diff.

Rules (from milestones.md §2.1):

* JSON is compared structurally. Object key order never matters. Array order
  matters unless a path is declared a set.
* A volatile value may be hidden only by a *typed* mask on an exact path. The
  masked value must still validate against its declared type; if it does not,
  the mask records a violation instead of hiding the value.
* Per-side run variables (agent ID, port, token, sandbox root) are replaced by
  placeholders by exact literal substitution, never by pattern matching.

Paths are lists of segments. A segment is an object key, an integer index,
"*" (every element or value), or "@json" (parse the current string as JSON).
"""

from __future__ import annotations

import copy
import json
import re
from typing import Any, Callable, Iterable

Path = list

_RFC3339 = re.compile(
    r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$"
)
_UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
_HEX = re.compile(r"^[0-9a-f]+$")


def _is_int(v: Any) -> bool:
    return isinstance(v, int) and not isinstance(v, bool)


MASK_TYPES: dict[str, Callable[[Any], bool]] = {
    "string": lambda v: isinstance(v, str),
    "nonempty-string": lambda v: isinstance(v, str) and v != "",
    "rfc3339": lambda v: isinstance(v, str) and bool(_RFC3339.match(v)),
    "uuid": lambda v: isinstance(v, str) and bool(_UUID.match(v)),
    "hex": lambda v: isinstance(v, str) and bool(_HEX.match(v)),
    "int": _is_int,
    "non-negative-int": lambda v: _is_int(v) and v >= 0,
    "number": lambda v: (isinstance(v, (int, float)) and not isinstance(v, bool)),
    "bool": lambda v: isinstance(v, bool),
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
}


class MaskError(ValueError):
    pass


def substitute(value: Any, variables: dict[str, str]) -> Any:
    """Replace known per-side literals with ${NAME} placeholders.

    Longer literals are replaced first so that a sandbox root is replaced
    before a shorter value that it happens to contain.
    """
    ordered = sorted(
        ((lit, name) for name, lit in variables.items() if lit),
        key=lambda item: -len(item[0]),
    )

    def walk(v: Any) -> Any:
        if isinstance(v, str):
            for lit, name in ordered:
                if lit in v:
                    v = v.replace(lit, "${" + name + "}")
            return v
        if isinstance(v, list):
            return [walk(x) for x in v]
        if isinstance(v, dict):
            return {walk(k): walk(x) for k, x in v.items()}
        return v

    return walk(value)


def _apply_at(doc: Any, path: Path, fn: Callable[[Any], Any]) -> tuple[Any, int]:
    """Apply fn to every value addressed by path. Returns (doc, hit_count)."""
    if not path:
        return fn(doc), 1
    head, rest = path[0], path[1:]
    hits = 0
    if head == "@json":
        if not isinstance(doc, str):
            return doc, 0
        try:
            parsed = json.loads(doc)
        except ValueError:
            return doc, 0
        new, hits = _apply_at(parsed, rest, fn)
        return {"$json": new}, hits
    if head == "*":
        if isinstance(doc, list):
            out = []
            for item in doc:
                new, n = _apply_at(item, rest, fn)
                out.append(new)
                hits += n
            return out, hits
        if isinstance(doc, dict):
            out = {}
            for key, item in doc.items():
                new, n = _apply_at(item, rest, fn)
                out[key] = new
                hits += n
            return out, hits
        return doc, 0
    if isinstance(head, int):
        if isinstance(doc, list) and -len(doc) <= head < len(doc):
            doc = list(doc)
            doc[head], hits = _apply_at(doc[head], rest, fn)
        return doc, hits
    if isinstance(doc, dict) and head in doc:
        doc = dict(doc)
        doc[head], hits = _apply_at(doc[head], rest, fn)
    return doc, hits


def parse_embedded(doc: Any, paths: Iterable[Path]) -> Any:
    """Expand JSON-in-string fields (for example MCP text content)."""
    for path in paths:
        doc, _ = _apply_at(doc, list(path) + ["@json"], lambda v: v)
    return doc


def apply_masks(doc: Any, masks: Iterable[dict]) -> tuple[Any, list[dict]]:
    """Apply typed masks. Returns the masked doc and a list of violations."""
    violations: list[dict] = []
    for mask in masks:
        kind = mask.get("type")
        if kind not in MASK_TYPES:
            raise MaskError(f"unknown mask type {kind!r}")
        if not str(mask.get("reason", "")).strip():
            raise MaskError(f"mask {mask.get('path')} has no reason")
        path = list(mask["path"])
        check = MASK_TYPES[kind]

        def fn(value: Any, _kind=kind, _check=check, _path=path) -> Any:
            if _check(value):
                return {"$masked": _kind}
            violations.append({"path": _path, "type": _kind, "value": value})
            return {"$maskViolation": _kind, "value": value}

        doc, hits = _apply_at(doc, path, fn)
        if hits == 0 and not mask.get("optional", False):
            violations.append({"path": path, "type": kind, "value": None, "reason": "mask path not found"})
    return doc, violations


def _set_key(v: Any) -> str:
    return json.dumps(v, sort_keys=True, separators=(",", ":"))


def apply_sets(doc: Any, set_paths: Iterable[Path]) -> Any:
    for path in set_paths:
        doc, _ = _apply_at(
            doc, list(path), lambda v: sorted(v, key=_set_key) if isinstance(v, list) else v
        )
    return doc


def canonical_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, indent=1, ensure_ascii=False)


def diff(left: Any, right: Any, path: Path | None = None, limit: int = 200) -> list[dict]:
    """Structural diff. Returns at most `limit` differences."""
    path = path or []
    out: list[dict] = []

    def walk(a: Any, b: Any, p: Path) -> None:
        if len(out) >= limit:
            return
        if type(a) is not type(b) and not (
            isinstance(a, (int, float)) and isinstance(b, (int, float))
            and not isinstance(a, bool) and not isinstance(b, bool)
        ):
            out.append({"path": p, "left": a, "right": b})
            return
        if isinstance(a, dict):
            for key in sorted(set(a) | set(b)):
                if key not in a:
                    out.append({"path": p + [key], "left": "$absent", "right": b[key]})
                elif key not in b:
                    out.append({"path": p + [key], "left": a[key], "right": "$absent"})
                else:
                    walk(a[key], b[key], p + [key])
            return
        if isinstance(a, list):
            if len(a) != len(b):
                out.append({"path": p, "left": f"$len={len(a)}", "right": f"$len={len(b)}"})
            for i, (x, y) in enumerate(zip(a, b)):
                walk(x, y, p + [i])
            return
        if a != b:
            out.append({"path": p, "left": a, "right": b})

    walk(left, right, path)
    return out


def normalize(observation: Any, spec: dict, variables: dict[str, str]) -> tuple[Any, list[dict]]:
    """Full pipeline for one side: substitute → parse embedded → sets → masks."""
    doc = substitute(copy.deepcopy(observation), variables)
    doc = parse_embedded(doc, spec.get("parseJson", []))
    doc = apply_sets(doc, spec.get("sets", []))
    return apply_masks(doc, spec.get("masks", []))
