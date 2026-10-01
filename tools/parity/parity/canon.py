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


def apply_omitempty(doc: Any, rules: Iterable[dict]) -> Any:
    """Go `omitempty` fields: absent and the zero value encode the same
    information, so fill each listed key with its zero value where absent.
    A rule needs a reason; it never hides a non-zero value."""
    for rule in rules:
        if not str(rule.get("reason", "")).strip():
            raise MaskError(f"omitempty rule {rule.get('path')} has no reason")

        def fill(obj: Any, _rule=rule) -> Any:
            if isinstance(obj, dict):
                return {**{k: _rule["zero"] for k in _rule["keys"]}, **obj}
            return obj

        doc, _ = _apply_at(doc, list(rule["path"]), fill)
    return doc


def normalize(observation: Any, spec: dict, variables: dict[str, str]) -> tuple[Any, list[dict]]:
    """Full pipeline for one side: substitute → parse embedded → omitempty → sets → masks."""
    doc = substitute(copy.deepcopy(observation), variables)
    doc = parse_embedded(doc, spec.get("parseJson", []))
    doc = apply_omitempty(doc, spec.get("omitempty", []))
    doc = apply_sets(doc, spec.get("sets", []))
    return apply_masks(doc, spec.get("masks", []))


# --- declared divergences -------------------------------------------------------
#
# A divergence is an approved, intentional difference between Go and Rust
# (milestones.md decision table). It is declared by id in a scenario's
# `compare.divergences` and defined once in tools/parity/divergences.json.
# Rules remove the same D8-owned content from both sides before the diff and
# apply only when the two sides are different implementations. A rule whose
# removed content is identical on both sides is stale: it hides nothing, so it
# must be deleted.


class DivergenceError(ValueError):
    pass


def _drop_matches(item: Any, rule: dict) -> bool:
    if not isinstance(item, dict):
        return False
    value = item.get(rule["field"])
    if "prefix" in rule:
        return isinstance(value, str) and value.startswith(rule["prefix"])
    if "in" in rule:
        return value in rule["in"]
    raise DivergenceError(f"drop rule needs prefix or in: {rule}")


def _split(value: Any, rule: dict) -> tuple[Any, Any]:
    """Return (kept, removed) for one addressed value."""
    if "drop" in rule:
        if not isinstance(value, list):
            return value, []
        kept = [x for x in value if not _drop_matches(x, rule["drop"])]
        removed = [x for x in value if _drop_matches(x, rule["drop"])]
        return kept, removed
    if "dropKeys" in rule:
        if not isinstance(value, dict):
            return value, {}
        keys = set(rule["dropKeys"])
        return ({k: v for k, v in value.items() if k not in keys},
                {k: v for k, v in value.items() if k in keys})
    if "dropLines" in rule:
        if not isinstance(value, str):
            return value, []
        needle = rule["dropLines"]["contains"]
        lines = value.split("\n")
        return ("\n".join(x for x in lines if needle not in x),
                [x for x in lines if needle in x])
    raise DivergenceError(f"divergence {rule.get('id')} has no drop, dropKeys or dropLines")


def load_divergences(path: Any) -> dict[str, dict]:
    rules = {}
    for rule in json.loads(path.read_text()):
        for key in ("id", "decision", "reason", "path"):
            if not rule.get(key):
                raise DivergenceError(f"divergence {rule.get('id')!r} has no {key}")
        if sum(k in rule for k in ("drop", "dropKeys", "dropLines")) != 1:
            raise DivergenceError(f"divergence {rule['id']} needs exactly one of drop, dropKeys, dropLines")
        if rule["id"] in rules:
            raise DivergenceError(f"duplicate divergence {rule['id']}")
        rules[rule["id"]] = rule
    return rules


def apply_divergences(left: Any, right: Any, ids: Iterable[str],
                      registry: dict[str, dict]) -> tuple[Any, Any, list[dict]]:
    """Remove declared divergent content from both sides.

    Returns the reduced documents and one record per declaration with the
    decision it cites and whether it is stale.
    """
    records = []
    for did in ids:
        rule = registry.get(did)
        if rule is None:
            raise DivergenceError(f"undefined divergence {did!r}")
        removed: dict[str, list] = {"a": [], "b": []}

        def take(side: str) -> Callable[[Any], Any]:
            def fn(value: Any) -> Any:
                kept, gone = _split(value, rule)
                removed[side].append(gone)
                return kept
            return fn

        left, _ = _apply_at(left, list(rule["path"]), take("a"))
        right, _ = _apply_at(right, list(rule["path"]), take("b"))
        records.append({"id": did, "decision": rule["decision"],
                        "stale": _set_key(removed["a"]) == _set_key(removed["b"])})
    return left, right, records
