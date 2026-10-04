## ADDED Requirements

### Requirement: Two named projections, chosen by destination, not a mode flag

Host Agent SHALL expose this capability as two separately named functions
rather than one function with a parameter: a delivery projection
(`redact_for_delivery`), used only for what is returned to the caller that
requested it — synchronously, or later through a task poll, live or
restored after a restart — and a storage projection
(`redact_for_storage`), used only for what reaches durable storage (an
audit row, or a stored echo of the caller's own arguments) and never
reaches any caller. A call site SHALL use exactly one, chosen by what that
value's destination is, not by a runtime flag threaded through a shared
function.

#### Scenario: A task's own delivered result is unaffected
- **WHEN** a task-aware capability's output schema contains a field with no `properties` entry or only a bare `additionalProperties: true`, and the call completes
- **THEN** the result returned through `tasks/get`, both immediately and after a restart, carries that field's real value, exactly as Go returns it

### Requirement: Durable projection fails closed on unmarked content

Host Agent SHALL persist a capability's JSON input or output into durable
storage (invocation arguments, task snapshots, structured results,
observations) only through its declared JSON Schema. A value SHALL reach a
durable sink in its original form only when some enclosing schema names it
explicitly: as a `properties` entry, or as a typed (object) schema under
`additionalProperties`. A key or value for which no such schema entry
exists — including one admitted only by a boolean `additionalProperties:
true`, or one whose enclosing schema is itself absent — SHALL be replaced
by a redaction marker instead of being copied. This rule composes with, and
does not weaken, write-only redaction: a field already marked `writeOnly:
true` SHALL still be redacted even when it is otherwise schema-covered.

#### Scenario: Argument outside the declared schema
- **WHEN** an accepted call carries an extra argument key that the tool's input schema does not declare in `properties` and does not cover with a typed `additionalProperties` schema
- **THEN** the durable invocation row records a redaction marker for that key, never the submitted value

#### Scenario: Open-schema structured result
- **WHEN** a capability's output schema marks part of its result `additionalProperties: true` with no further typing, and the live value populates that part
- **THEN** the durable result and observation rows record a redaction marker for that part, never the live value

#### Scenario: Schema-covered content is unaffected
- **WHEN** a value is covered by a `properties` entry or a typed `additionalProperties` schema and is not marked `writeOnly`
- **THEN** it is projected and persisted exactly as the schema describes, recursively

#### Scenario: Write-only fields stay redacted
- **WHEN** a schema-covered field is marked `writeOnly: true`
- **THEN** it is replaced by a redaction marker regardless of this requirement

### Requirement: Declared divergence from the pinned Go projection

The pinned Go reference (`internal/hostmcp/evidence_redaction.go`) preserves
unmarked content verbatim; this capability is a declared, owner-approved
divergence (decision D13) scoped to durable projection only. It SHALL NOT
change what any capability accepts from or returns to its caller, only what
is written to durable storage. The parity harness SHALL exclude the paths
this changes from Go-vs-Rust comparison via `tools/parity/divergences.json`
entries citing D13, and SHALL instead assert the Rust behavior directly
through a contract suite with Rust canaries.

#### Scenario: Canary proves the redaction is load-bearing
- **WHEN** a patched Rust build that reverts to passthrough-on-unmarked runs the `evidence-redaction` contract suite
- **THEN** the suite fails, proving the scenario actually exercises this requirement
