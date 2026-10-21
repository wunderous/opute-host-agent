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
    "host-reservation-id": lambda v: isinstance(v, str) and bool(
        re.fullmatch(r"host-reservation-[1-9][0-9]*-[1-9][0-9]*", v)),
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
                if lit not in v:
                    continue
                if lit.isdigit():
                    # A numeric literal (a port) is replaced only where it is
                    # not part of a longer token: a byte count that happens
                    # to contain the port's digits, or -- since the port is
                    # itself a decimal string -- a hex hash whose digit run
                    # coincidentally matches it, flanked by a-f letters that
                    # a digit-only boundary would not catch.
                    v = re.sub(r"(?<![0-9a-zA-Z])" + lit + r"(?![0-9a-zA-Z])", "${" + name + "}", v)
                else:
                    v = v.replace(lit, "${" + name + "}")
            return v
        if isinstance(v, list):
            return [walk(x) for x in v]
        if isinstance(v, dict):
            return {walk(k): walk(x) for k, x in v.items()}
        return v

    return walk(value)


# A numeric variable (a port) substituted where embedded JSON had a number.
_BARE_PLACEHOLDER = re.compile(r'(?<=[:\[,])\$\{([A-Z][A-Z0-9_]*)\}(?=[,\]}])')


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


def parse_embedded(doc: Any, paths: Iterable[Path], variables: dict[str, str] | None = None) -> Any:
    """Expand JSON-in-string fields (for example MCP text content).

    Substitution runs first and also rewrites a numeric variable (a port)
    that embedded JSON held as a bare number, which leaves it unparseable.
    Such a placeholder is put back as that side's own number, so the field
    is a number exactly as on the wire, where only strings are substituted.
    """
    numbers = {name: lit for name, lit in (variables or {}).items() if lit and lit.isdigit()}

    def parse(value: Any) -> Any:
        if not isinstance(value, str):
            return value
        try:
            return {"$json": json.loads(value)}
        except ValueError:
            pass
        restored = _BARE_PLACEHOLDER.sub(
            lambda m: numbers.get(m.group(1), m.group(0)), value)
        try:
            return {"$json": json.loads(restored)} if restored != value else value
        except ValueError:
            return value

    for path in paths:
        doc, _ = _apply_at(doc, list(path), parse)
    return doc


#: D13's storage-projection redaction marker (`crates/host-agent/src/
#: evidence.rs`'s `redact_for_storage`). A live-value mask accepts this
#: marker as automatically satisfying its declared type: a field that is
#: both masked (because its real value is volatile) and redacted (because
#: its schema does not cover it) would otherwise record a permanent mask
#: violation before any divergence ever runs -- masks apply per side during
#: `normalize()`, strictly before `apply_divergences` sees the combined
#: pair, so a later divergence declaration cannot retroactively clear it.
REDACTED_MARKER = "[redacted]"


def apply_masks(doc: Any, masks: Iterable[dict]) -> tuple[Any, list[dict]]:
    """Apply typed masks. Returns the masked doc and a list of violations."""
    violations: list[dict] = []
    for mask in masks:
        kind = mask.get("type")
        if kind not in MASK_TYPES and kind != "one-of":
            raise MaskError(f"unknown mask type {kind!r}")
        if not str(mask.get("reason", "")).strip():
            raise MaskError(f"mask {mask.get('path')} has no reason")
        path = list(mask["path"])
        if kind == "one-of":
            # A value chosen by a race between equal candidates: any declared
            # candidate is accepted, anything else is a violation.
            values = mask.get("values")
            if not isinstance(values, list) or len(values) < 2:
                raise MaskError(f"one-of mask {path} needs at least two values")
            check = (lambda v, _values=tuple(json.dumps(x, sort_keys=True) for x in values):
                     json.dumps(v, sort_keys=True) in _values)
        else:
            check = MASK_TYPES[kind]

        # "match": the mask covers only values that fully match this regex
        # (a random temp-file name among literal argv, say); other values at
        # the path are left to compare as they are.
        pattern = re.compile(mask["match"]) if "match" in mask else None

        def fn(value: Any, _kind=kind, _check=check, _path=path, _pattern=pattern) -> Any:
            if _pattern is not None and not (isinstance(value, str) and _pattern.fullmatch(value)):
                return value
            if _check(value) or value == REDACTED_MARKER:
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
    doc = parse_embedded(doc, spec.get("parseJson", []), variables)
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


def _matches_redacted(value: Any, literal: Any) -> bool:
    """A redacted leaf, bare or wrapped. A field that is both scenario-masked
    (a live, non-deterministic value) and redacted never reaches `diff` as
    the bare literal: masks run before divergences, so a value that fails
    its declared type check -- because it is now `literal`, not the typed
    value the mask expects -- is wrapped by `apply_masks` as
    `{"$maskViolation": kind, "value": literal}` instead."""
    if value == literal:
        return True
    return (
        isinstance(value, dict)
        and set(value) == {"$maskViolation", "value"}
        and value["value"] == literal
    )


def _drop_value_deep_paired(left: Any, right: Any, literal: Any) -> tuple[Any, Any, list, list]:
    """Recursively drop, from both sides together, every dict entry or list
    element whose *right*-side value is a redacted leaf (see
    `_matches_redacted`), at any depth, independent of key name or
    position. The matching left-side entry is dropped too, so both sides
    converge to the same residual shape; unlike a per-side drop (which only
    removes identically-named content), this lets one side's declared
    behavior change (always producing `literal`) hide the other side's
    differing, non-redacted content at that spot.
    """
    removed_left: list = []
    removed_right: list = []

    def walk(a: Any, b: Any) -> tuple[Any, Any]:
        if isinstance(b, dict):
            left_dict = a if isinstance(a, dict) else {}
            kept_a, kept_b = {}, {}
            for key in dict.fromkeys([*left_dict, *b]):
                in_a, in_b = key in left_dict, key in b
                av, bv = left_dict.get(key), b.get(key)
                if in_b and _matches_redacted(bv, literal):
                    removed_left.append({key: av} if in_a else None)
                    removed_right.append({key: bv})
                    continue
                na, nb = walk(av, bv)
                if in_a:
                    kept_a[key] = na
                if in_b:
                    kept_b[key] = nb
            return kept_a, kept_b
        if isinstance(b, list):
            left_list = a if isinstance(a, list) else []
            out_a, out_b = [], []
            for i, bv in enumerate(b):
                av = left_list[i] if i < len(left_list) else None
                if _matches_redacted(bv, literal):
                    removed_left.append(av if i < len(left_list) else None)
                    removed_right.append(bv)
                    continue
                na, nb = walk(av, bv)
                out_b.append(nb)
                if i < len(left_list):
                    out_a.append(na)
            return out_a, out_b
        return a, b

    new_left, new_right = walk(left, right)
    return new_left, new_right, removed_left, removed_right


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
    if "keepPrefix" in rule:
        # A string keeps only its declared prefix; the rest is the declared
        # difference. A string without the prefix stays compared whole.
        prefix = rule["keepPrefix"]
        if not isinstance(value, str) or not value.startswith(prefix):
            return value, ""
        return prefix, value[len(prefix):]
    raise DivergenceError(f"divergence {rule.get('id')} has no drop, dropKeys, dropLines or keepPrefix")


# --- renameToGo: a declared rename on the Rust side only ------------------------
#
# D19 (neutral-host-tool-names): Rust publishes provider-neutral names where
# Go publishes provider-named ones, renames one input field, and so publishes
# different catalog revisions. The rule rewrites the Rust observation back to
# Go's spelling using the one name map the Rust catalog itself is built from
# (crates/host-agent/catalog/neutral-names.json) and the revision pairs pinned
# in parity-manifest.json. It changes nothing else: a neutral name is replaced
# only as a whole word, the field only inside the renamed tool's input
# schema, and a revision only when it is a pinned Rust revision. A list that
# was sorted by name (or by a name-bearing key) and whose order the rename
# changed is sorted again, since Go sorts the same list under its own names.
# The rule is stale when it rewrites nothing.

_REPO = __import__("pathlib").Path(__file__).resolve().parents[3]
_WORD = re.compile(r"[A-Za-z0-9_]")
# A wire name is "<prefix>_<name>"; after substitution the prefix is this
# placeholder, and the name after it is still a whole name.
_WIRE_PREFIX = "${TOOL_PREFIX}_"
_SORT_KEYS = ("name", "operationId", "tool", "sourceTool", "targetTool")


def _rename_tables(rule: dict) -> tuple[dict, dict, dict]:
    spec = rule["renameToGo"]
    names = json.loads((_REPO / spec["names"]).read_text())
    to_go = {neutral: go for go, neutral in names["tools"].items()}
    # A tool that keeps its Go name may still rename a field (D21).
    fields = {names["tools"].get(go, go): {new: old for old, new in renamed.items()}
              for go, renamed in names.get("fields", {}).items()}
    manifest = json.loads((_REPO / spec["revisions"]).read_text())
    pinned = manifest["catalogRevisions"]
    pairs = [pair for pair in pinned.values() if isinstance(pair, dict)]
    pairs += pinned.get("providerFixtures", [])
    revisions = {pair["rust"]: pair["go"] for pair in pairs if pair.get("rust") and pair.get("go")}
    # D23: selector values a tool publishes neutrally, in its input schema's
    # enums and in result keys.
    section = names.get("values", {})
    values = {"tools": section.get("tools", {}), "resultFields": set(section.get("resultFields", [])),
              "toGo": {published: go for go, published in section.get("map", {}).items()}}
    return to_go, fields, revisions, values


def _rename_text(text: str, to_go: dict, revisions: dict) -> str:
    out, i = [], 0
    names = sorted(to_go, key=len, reverse=True)
    while i < len(text):
        if i == 0 or not _WORD.match(text[i - 1]) or text.endswith(_WIRE_PREFIX, 0, i):
            for neutral in names:
                end = i + len(neutral)
                if text.startswith(neutral, i) and (end == len(text) or not _WORD.match(text[end])):
                    out.append(to_go[neutral])
                    i = end
                    break
            else:
                out.append(text[i])
                i += 1
            continue
        out.append(text[i])
        i += 1
    renamed = "".join(out)
    for rust, go in revisions.items():
        renamed = renamed.replace(rust, go)
    return renamed


def _sorted_by(items: list, key: str | None) -> bool:
    values = [x if key is None else x.get(key) for x in items]
    if any(not isinstance(v, str) for v in values):
        return False
    return values == sorted(values)


def _rename_to_go(doc: Any, tables: tuple[dict, dict, dict, dict]) -> Any:
    to_go, fields, revisions, values = tables

    def walk(value: Any) -> Any:
        if isinstance(value, str):
            return _rename_text(value, to_go, revisions)
        if isinstance(value, list):
            renamed = [walk(x) for x in value]
            if renamed != value and len(value) > 1:
                if all(isinstance(x, str) for x in value) and _sorted_by(value, None):
                    return sorted(renamed)
                if all(isinstance(x, dict) for x in value):
                    # Sorted by every name-bearing key present, in order: a
                    # key that is constant across the list (each edge's
                    # sourceTool, say) leaves the order to the next one.
                    keys = [k for k in _SORT_KEYS if all(k in x for x in value)]
                    if keys and all(isinstance(x[k], str) for x in value for k in keys):
                        tuples = [tuple(x[k] for k in keys) for x in value]
                        if tuples == sorted(tuples):
                            return sorted(renamed, key=lambda x: tuple(x[k] for k in keys))
                    for key in _SORT_KEYS:
                        if all(key in x for x in value) and _sorted_by(value, key):
                            return sorted(renamed, key=lambda x: x[key])
            return renamed
        if isinstance(value, dict):
            name = value.get("name")
            if isinstance(name, str) and name.startswith(_WIRE_PREFIX):
                name = name[len(_WIRE_PREFIX):]
            renamed_fields = fields.get(name) if isinstance(name, str) else None
            valued = values["tools"].get(name) if isinstance(name, str) else None
            out = {}
            for key, item in value.items():
                if renamed_fields and key == "inputSchema" and isinstance(item, dict):
                    item = _rename_schema_fields(item, renamed_fields)
                if valued and key == "inputSchema" and isinstance(item, dict):
                    item = _rename_schema_values(item, valued, values["toGo"])
                if key in values["resultFields"] and isinstance(item, str) and item in values["toGo"]:
                    item = values["toGo"][item]
                out[_rename_text(key, to_go, revisions)] = walk(item)
            return out
        return value

    return walk(doc)


def _rename_schema_fields(schema: dict, renamed: dict) -> dict:
    schema = dict(schema)
    if isinstance(schema.get("properties"), dict):
        schema["properties"] = {renamed.get(k, k): v for k, v in schema["properties"].items()}
    if isinstance(schema.get("required"), list):
        schema["required"] = [renamed.get(k, k) if isinstance(k, str) else k for k in schema["required"]]
    return schema


def _rename_schema_values(schema: dict, fields: list, to_go: dict) -> dict:
    properties = schema.get("properties")
    if not isinstance(properties, dict):
        return schema
    properties = dict(properties)
    for field in fields:
        prop = properties.get(field)
        if isinstance(prop, dict) and isinstance(prop.get("enum"), list):
            properties[field] = {**prop, "enum": [to_go.get(v, v) if isinstance(v, str) else v
                                                  for v in prop["enum"]]}
    return {**schema, "properties": properties}


_KINDS = ("drop", "dropKeys", "dropLines", "dropValueDeep", "keepPrefix", "renameToGo")


def load_divergences(path: Any) -> dict[str, dict]:
    rules = {}
    for rule in json.loads(path.read_text()):
        for key in ("id", "decision", "reason", "path"):
            if not rule.get(key):
                raise DivergenceError(f"divergence {rule.get('id')!r} has no {key}")
        if sum(k in rule for k in _KINDS) != 1:
            raise DivergenceError(f"divergence {rule['id']} needs exactly one of {', '.join(_KINDS)}")
        if rule["id"] in rules:
            raise DivergenceError(f"duplicate divergence {rule['id']}")
        rules[rule["id"]] = rule
    return rules


def apply_divergences(left: Any, right: Any, ids: Iterable[str],
                      registry: dict[str, dict], rust_side: str = "b") -> tuple[Any, Any, list[dict]]:
    """Remove declared divergent content from both sides.

    Returns the reduced documents and one record per declaration with the
    decision it cites and whether it is stale. A `renameToGo` rule rewrites
    only `rust_side` ("a" or "b").
    """
    records = []
    for did in ids:
        rule = registry.get(did)
        if rule is None:
            raise DivergenceError(f"undefined divergence {did!r}")
        if "renameToGo" in rule:
            tables = _rename_tables(rule)
            target = right if rust_side == "b" else left
            before = copy.deepcopy(target)
            target, _ = _apply_at(target, list(rule["path"]), lambda v: _rename_to_go(v, tables))
            if rust_side == "b":
                right = target
            else:
                left = target
            records.append({"id": did, "decision": rule["decision"], "stale": target == before})
            continue
        if "dropValueDeep" in rule:
            captured_left: list = []
            captured_right: list = []
            _apply_at(left, list(rule["path"]), lambda v: captured_left.append(v) or v)
            _apply_at(right, list(rule["path"]), lambda v: captured_right.append(v) or v)
            sub_left = captured_left[0] if captured_left else None
            sub_right = captured_right[0] if captured_right else None
            new_left, new_right, removed_left, removed_right = _drop_value_deep_paired(
                sub_left, sub_right, rule["dropValueDeep"])
            left, _ = _apply_at(left, list(rule["path"]), lambda _v, _n=new_left: _n)
            right, _ = _apply_at(right, list(rule["path"]), lambda _v, _n=new_right: _n)
            records.append({"id": did, "decision": rule["decision"],
                            "stale": _set_key(removed_left) == _set_key(removed_right)})
            continue
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
