# Host Agent contract parity

## Purpose

Define the externally observable Host Agent behavior that the Rust
implementation must preserve from the pinned Go source, including the MCP
surface, identity, typed execution, durable outcomes, and operator workflows.

## Requirements

### Requirement: No capability change during the language migration

The Rust Host Agent SHALL preserve every supported public capability and
operator-visible behavior of the pinned Go baseline. It SHALL neither add nor
remove a product capability, endpoint, mode, provider integration, or default
as part of this migration. Fine-grained names, schemas, effects, errors, and
availability are governed by the pinned typed source contracts and a captured
catalog for each supported mode and provider combination, not by a memorized
tool count.

#### Scenario: Compare a supported catalog
- **WHEN** Go and Rust start with equivalent configuration and active providers in isolated fixtures
- **THEN** their authenticated catalogs expose compatible names, descriptions, input and output schemas, effects, approval requirements, idempotency declarations, and resource edges at the same contract revision

#### Scenario: Unsupported or new capability appears
- **WHEN** a Rust catalog adds a name or omits a name present in the equivalent Go catalog without a separately approved contract change
- **THEN** the migration parity gate fails and no cutover is authorized

### Requirement: One exact Host Agent identity

The Host Agent SHALL require one explicit opaque `OPUTE_REMOTE_AGENT_ID` and
SHALL use its exact value for routing, ownership, sessions, inventory, and
canonicalization. Hostnames, fingerprints, provider IDs, and display names
SHALL remain evidence rather than identity substitutes. Ambiguous or missing
identity SHALL fail closed.

#### Scenario: Identity is missing
- **WHEN** startup lacks `OPUTE_REMOTE_AGENT_ID`
- **THEN** the agent rejects startup before exposing a usable MCP listener

#### Scenario: Two agents share a physical machine
- **WHEN** Go and Rust run concurrently during validation on the same machine
- **THEN** they use distinct explicit IDs, endpoints, credentials, and durable state, and neither ID is inferred from the machine

### Requirement: Compatible authenticated MCP surface

The Host Agent SHALL preserve the pinned Go Streamable HTTP MCP behavior,
including protocol revision `2026-07-28`, authenticated `server/discover`,
`tools/list`, `tools/call`, structured results and errors, task state, and
cancellation. `/health` SHALL remain an open liveness endpoint, while `/mcp`
SHALL reject missing or invalid credentials under the configured auth mode.
The opt-in legacy handshake exception SHALL remain default-off and limited to
the exact method set authorized by Go ADR 0011; it SHALL NOT waive validation
for modern `server/discover` or `tasks/*` methods.

#### Scenario: Read-only authenticated first call
- **WHEN** a client presents a valid configured credential and performs discovery, tool listing, and `get_host_info {}`
- **THEN** Rust returns the same contract shape and read-only outcome as Go without enabling mutations

#### Scenario: Invalid bearer token
- **WHEN** a client calls `/mcp` with a wrong bearer token
- **THEN** Rust rejects it with the same authentication status and no tool execution

#### Scenario: Legacy exception remains bounded
- **WHEN** legacy handshake mode is explicitly enabled and a request invokes a modern-only method without modern protocol metadata
- **THEN** Rust rejects that request under the same bounded rule as Go

### Requirement: Compatible modes, configuration, and distribution

The Host Agent SHALL preserve the documented standalone and platform serving
modes, their bind and port defaults, the CLI entrypoints and configuration
precedence, and the observable launcher, installer, and release-artifact
contracts from the pinned baseline. Native host execution support SHALL remain
Linux and WSL2 for this migration. A packaging rewrite SHALL NOT silently
change commands, environment names, artifact names, or client setup.

#### Scenario: Standalone defaults
- **WHEN** an operator starts the agent with the documented standalone defaults
- **THEN** it binds the documented loopback endpoint and keeps mutations denied by default

#### Scenario: Platform defaults
- **WHEN** an enrolled operator starts the platform mode with valid identity and credentials
- **THEN** it uses the documented platform bind and port behavior without changing Platform ownership of intent or routing

### Requirement: Typed target and effect admission

The Host Agent SHALL resolve tenant-scoped canonical resource URIs and resource
kinds before execution, apply the same authorization, approval, effect, and
mutation gates as Go, and pass tool-specific arguments to the owning capability
without guessing or rewriting IDs. `vm:` and `container:` targets SHALL remain
distinct. No success result may be invented from a rejected or unknown target.

#### Scenario: Mutation gate remains closed
- **WHEN** a standalone client calls a mutation while the explicit mutation gate is disabled
- **THEN** Rust denies the call before side effects with a compatible typed error

#### Scenario: Runtime kind differs
- **WHEN** an operation requires a container but the canonical target resolves to a virtual machine
- **THEN** Rust rejects the mismatch before side effects instead of coercing the URI or choosing another executor

### Requirement: Revisioned provider-neutral catalog and lifecycle

The Host Agent SHALL expose neutral capability descriptors derived from the
active provider generations. Candidate providers SHALL remain isolated until
their manifest, schema, dependency, catalog, and readiness checks pass; failed
activation SHALL leave the prior active generation available. In-flight work
SHALL remain bound to the generation that accepted it, and disposal SHALL be
bounded, idempotent, and reverse-ordered.

#### Scenario: Candidate activation fails
- **WHEN** a candidate provider fails readiness or catalog publication
- **THEN** the active catalog and generation remain usable and Rust reports the failed candidate truthfully

#### Scenario: Active generation changes
- **WHEN** a ready candidate becomes active while an earlier operation is running
- **THEN** the earlier operation retains its original generation and new work receives the new catalog revision

### Requirement: One typed plan and recipe execution path

The Host Agent SHALL preserve Go's versioned recipe validation, canonical
hashing, plan extraction, single generic executor, durable node outcomes,
readiness assertions, bounded retry, cancellation, recovery, and declared
compensation behavior. A provider SHALL NOT create a second workflow runner or
bypass plan admission with a callback.

#### Scenario: Recipe validation
- **WHEN** a client supplies a supported recipe with invalid bindings or a stale catalog revision
- **THEN** Rust rejects it before side effects with a compatible typed validation outcome

#### Scenario: Interrupted plan
- **WHEN** a running plan is cancelled or the process restarts
- **THEN** Rust exposes the truthful durable state and only resumes, compensates, or stops according to the same plan contract as Go

### Requirement: Durable and redacted observations

The Host Agent SHALL preserve durable operation, task, plan, provider
generation, and observation semantics across restart and cutover. Durable
records SHALL use schema-derived redaction for secrets; unknown projections
SHALL fail closed. Migrating existing local state SHALL preserve record meaning
and provide a verified rollback path before Rust becomes its writer.

#### Scenario: Secret-bearing input
- **WHEN** a capability accepts a write-only credential field
- **THEN** the value is available only to its authorized transient execution binding and is absent from durable records, client-visible evidence, and logs

#### Scenario: State migration cannot be verified
- **WHEN** an existing Go state copy cannot be migrated and compared without loss or ambiguity
- **THEN** Rust does not replace the production state writer and the Go data remains recoverable

### Requirement: Host Agent and Platform ownership stays separate

The Host Agent SHALL execute explicit typed assignments and report observed
outcomes. Opute Platform and its LLM layer SHALL retain intent, authorization
decisions, cross-host orchestration, target assignment, routing, and semantic
claims about user-request completion. A successful Host Agent tool call SHALL
NOT itself assert that a natural-language request was satisfied.

#### Scenario: Tool call succeeds
- **WHEN** a Host Agent tool returns a successful typed result
- **THEN** Rust records the result and evidence without inventing a Platform or model-owned semantic completion decision

### Requirement: Cutover requires boundary-matched parity evidence

The Go implementation SHALL remain the production owner until the Rust
implementation passes catalog, schema, CLI, auth, MCP wire, provider, plan,
durable-state, package, client, and isolated external-effect parity gates for
the selected Go revision. A missing or blocked gate SHALL be recorded as
unverified, not converted into a pass. Cutover and Go retirement SHALL be
separate, reversible decisions.

#### Scenario: Unit tests pass but wire evidence is absent
- **WHEN** Rust unit and contract suites pass but authenticated MCP or state migration evidence is missing
- **THEN** the cutover gate remains closed

#### Scenario: Rollback window ends
- **WHEN** Rust has served the full surface through the agreed rollback window and required evidence remains green
- **THEN** the owner may make a separate decision to retire Go artifacts and registrations
