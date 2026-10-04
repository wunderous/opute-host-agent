## 1. Rust: two named projections

- [x] 1.1 Split `redact_by_schema` into `redact_for_delivery` (Go's
  original behavior: only `writeOnly` hidden) and `redact_for_storage`
  (fail-closed: a value with no concrete object schema -- no `properties`
  entry, no typed-object `additionalProperties`, including a boolean
  `additionalProperties: true` or an absent schema -- is replaced by a
  redaction marker instead of being recursed into and copied).
- [x] 1.2 Route `redact_task_result` (what `tasks/get` delivers, live and
  restored) through delivery; route `redact_task_args` and
  `record_invocation`'s audit row through storage.
- [x] 1.3 Unit tests covering both projections independently: storage --
  an extra argument key outside `properties` with no `additionalProperties`;
  a value under `additionalProperties: true`; a `properties`-covered nested
  object (unaffected); a `writeOnly` field nested inside a typed
  `additionalProperties` schema (still redacted); an array whose `items`
  schema is absent. Delivery -- an open/undeclared field survives while a
  `writeOnly` field is still hidden, on the same inputs.

## 2. Harness: divergence, contract suite, canaries

- [x] 2.1 `tools/parity/divergences.json`: add `D13.*` entries for the
  durable paths this changes, citing decision `D13`.
- [x] 2.2 `tools/parity/contracts/evidence-redaction.json`: single-
  implementation contract scenarios asserting Rust redacts each of the
  three cases from `evidence/m5/unknown-projection/outcomes.json` and still
  projects schema-covered, non-write-only content unchanged. No catalog
  tool currently combines task-awareness with an open-schema output field
  to exercise the delivery projection end-to-end; `redact_for_delivery`'s
  behavior is proven at the unit level (`tools.rs`) instead -- see 1.3.
- [x] 2.3 `tools/parity/rust-canaries.json`: a patched build that reverts
  `redact_for_storage` to passthrough-on-unmarked must turn the
  `evidence-redaction` contract suite red (K24).

## 3. Evidence and milestone record

- [x] 3.1 Re-run `python3 -m parity unknown-projection` against the updated
  Rust binary; the three previously-verbatim canaries now come back
  redacted, Go's pinned verbatim persistence recorded as the expected,
  declared-divergent baseline.
- [x] 3.2 `make parity-verify-m5`: gate m5 passes.
- [x] 3.3 Update `evidence/m5/README.md`'s unknown-projection row and
  `openspec/changes/reimplement-host-agent-in-rust/milestones.md`'s D13 row
  to "Decided (owner, 2026-10-04)", describing the two-projection split and
  citing this change and `tools/parity/contracts/evidence-redaction.json`.
