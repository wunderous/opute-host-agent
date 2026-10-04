# Fail-closed redaction of unmarked durable projections

## Why

M5's durable-state evidence work found that the pinned Go projection
(`internal/hostmcp/evidence_redaction.go`) preserves a value verbatim
whenever no schema entry marks it: an argument key absent from both
`properties` and `additionalProperties`, or a value whose parent schema
declares `additionalProperties: true` with no further typing. Three
concrete, reproducible cases (`tools/parity/parity/unknown_projection.py`,
`evidence/m5/unknown-projection/outcomes.json`) persist an injected canary
verbatim into durable rows on both Go and Rust: an extra `get_host_info`
argument, an extra `request_task_input` argument, and `get_vm_info`'s
open-schema `state.incusStatus` shim measurement.

The owner reviewed this as a product decision (not a parity bug) and chose
to make the durable-projection contract stricter rather than carry Go's gap
forward: unmarked content must never reach a durable sink verbatim, matching
the fail-closed precedent the Rust port already uses for unknown
capabilities and undeclared plan outputs (`redact_plan_run_state`'s
`None => {"redacted": true}` arm). The Go agent is not changed by this work
(it stays the pinned, frozen reference); the deviation is declared, scoped
to projection redaction, and verified by its own contract suite, the same
shape D8 (`secure-oauth-issuance`) already established.

## What Changes

- `crates/host-agent/src/evidence.rs`'s single `redact_by_schema` is split
  into two named policies, not one function with a mode flag — a first
  single-policy version was built and then corrected once review found it
  would have also hidden content from a caller who already owns it (see
  below):
  - `redact_for_delivery`: Go's original, unchanged behavior. Only
    `writeOnly` is ever hidden; "no schema entry for this key" and
    "`additionalProperties: true`" still pass through exactly as Go
    returns them. Used for `redact_task_result`, the function that builds
    a task-aware tool's own completed result — delivered through
    `tasks/get`, live and after a restart. That caller already owns this
    data through its one delivery channel; withholding it has no security
    benefit and is pure functionality loss.
  - `redact_for_storage`: the stricter, fail-closed policy. A value is
    projected in its original form only when a `properties` entry or a
    typed (object) `additionalProperties` schema names it explicitly;
    anything else — including open-but-untyped content — is replaced
    wholesale. Used for `record_invocation`'s durable audit row
    (`capability_invocations`) and `redact_task_args`'s stored echo of the
    caller's own arguments (`operations.task_snapshot_json.toolArgs`) —
    neither is delivered to anyone, and an argument echo costs nothing to
    redact since the caller already holds whatever they sent.
- An object schema's own declared `properties` entries, and a typed (object)
  `additionalProperties` schema, are unaffected under either policy: those
  remain schema-covered and continue to be redacted only where
  `writeOnly: true` marks them.
- **Declared divergence (D13).** Rust's storage projection is now stricter
  than Go's for unmarked/open-schema content; its delivery projection is
  unchanged. `tools/parity/divergences.json` gains `D13.*` entries for the
  durable paths this changes; the parity harness excludes them from
  Go-vs-Rust comparison and instead asserts the Rust behavior directly via
  a contract suite with Rust canaries.

## Capabilities

### New Capabilities

- `evidence-redaction`: the fail-closed rule for projecting arbitrary
  capability input/output JSON into durable storage.

### Modified Capabilities

- `host-agent-contract`: "No capability change during the language
  migration" names `D13` alongside `D8` as an owner-approved, declared
  divergence.

## Non-goals

- No change to the Go Host Agent or its pinned evidence redaction.
- No change to `writeOnly` handling, to the unknown-capability wholesale
  redaction path, or to `redact_plan_run_state`/`redact_plan_document`
  beyond the shared `redact_by_schema` primitive they already call.
- No new product capability, endpoint, mode, or default — this narrows what
  is persisted; it does not change what any capability accepts or returns
  to its caller. (The storage-only policy is why two functions exist rather
  than one: a single stricter default would have also changed what a
  task-aware tool returns through `tasks/get`, which this change explicitly
  avoids.)

## Impact

- **Rust:** `crates/host-agent/src/evidence.rs` (`redact_by_schema`), and
  every durable row it feeds (`capability_invocations.arguments_json`,
  `operations.task_snapshot_json`, `capability_invocations.result_json`,
  observation envelopes).
- **Parity harness:** three new `D13.*` divergence declarations, a
  `tools/parity/contracts/evidence-redaction.json` contract suite, and
  Rust canaries proving the redaction is load-bearing.
- **Operators/providers:** none. Rejected/redacted fields were never
  contractually guaranteed content; this only removes previously-leaking
  unmarked bytes from durable storage.
