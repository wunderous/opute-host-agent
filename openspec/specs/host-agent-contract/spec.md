# Host Agent contract

## Purpose

Define the externally observable Host Agent behavior: the MCP surface,
identity, typed execution, durable outcomes, and operator workflows. This
specification, the accepted decisions, and the frozen v0.2.4 contract record
as evidence (D27, D28) are the authority for the Rust implementation.

## Requirements

### Requirement: No capability change during the language migration

The Rust Host Agent SHALL provide every public capability and operator-visible
behavior recorded in the frozen v0.2.4 contract record (`baseline/inventory`,
`baseline/source-lock.json`), as amended by the accepted decisions. It SHALL
neither add nor remove a product capability, endpoint, mode, provider
integration, or default without a separately accepted OpenSpec change. Fine-grained
names, schemas, effects, errors, and availability are governed by the recorded
typed contracts and the captured catalog for each supported mode and provider
combination, not by a memorized tool count. A behavior the record does not
decide SHALL be raised as an owner question, not settled by consulting the Go
implementation (decision D27).

The prose of this specification and the accepted decisions are normative. The
frozen record is evidence: it decides only what the prose leaves to it, and a
field it captured incidentally is not a requirement (decision D28, Q0). Unless
a requirement states an order, the order of items in a list is unspecified
(Q6). Error and log message text is not contract (Q8), and neither are
incidental MCP SDK details such as session log lines or server information
beyond its name (Q11).

#### Scenario: Compare a supported catalog
- **WHEN** Rust starts with a supported configuration and active providers in an isolated fixture
- **THEN** its authenticated catalog exposes the names, descriptions, input and output schemas, effects, approval requirements, idempotency declarations, and resource edges of the recorded catalog for that mode and provider combination, as amended by accepted decisions

#### Scenario: Unsupported or new capability appears
- **WHEN** a Rust catalog adds a name or omits a recorded name without an accepted contract change
- **THEN** the contract gate fails and no cutover is authorized

#### Scenario: A list's order differs
- **WHEN** a result lists the same items as the record in a different order and no requirement states an order
- **THEN** the result satisfies the contract

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
- **WHEN** two Host Agents run concurrently on the same machine
- **THEN** they use distinct explicit IDs, endpoints, credentials, and durable state, and neither ID is inferred from the machine

### Requirement: Compatible authenticated MCP surface

The Host Agent SHALL serve the recorded Streamable HTTP MCP surface, including
protocol revision `2026-07-28`, authenticated `server/discover`, `tools/list`,
`tools/call`, structured results and errors, task state, and cancellation.
`/health` SHALL remain an open liveness endpoint, while `/mcp` SHALL reject
missing or invalid credentials under the configured auth mode. The opt-in
legacy handshake exception SHALL remain default-off and limited to the
methods `initialize`, `notifications/initialized`, `notifications/cancelled`,
`ping`, `tools/list`, `tools/call`, `prompts/list` and `prompts/get` (Q2); it
SHALL NOT waive validation for modern `server/discover`, `tasks/*` or
`resources/*` methods.

An authentication refusal's HTTP status code is contract; its body and message
text are not (Q1). A typed admission or capability error's code is contract;
its message is not (Q3).

#### Scenario: Read-only authenticated first call
- **WHEN** a client presents a valid configured credential and performs discovery, tool listing, and `get_host_info {}`
- **THEN** Rust returns the recorded contract shape and a read-only outcome without enabling mutations

#### Scenario: Invalid bearer token
- **WHEN** a client calls `/mcp` with a wrong bearer token
- **THEN** Rust rejects it with the recorded authentication status code and no tool execution

#### Scenario: Legacy exception remains bounded
- **WHEN** legacy handshake mode is explicitly enabled and a request invokes a modern-only method without modern protocol metadata
- **THEN** Rust rejects that request under the recorded bounded rule

#### Scenario: A refusal's message changes
- **WHEN** a typed refusal keeps its code but its message text differs from the record
- **THEN** the result satisfies the contract

### Requirement: Compatible modes, configuration, and distribution

The Host Agent SHALL keep the documented standalone and platform serving
modes, their bind and port defaults, the CLI entrypoints and configuration
precedence, and the recorded launcher, installer, and release-artifact
contracts. Native host execution support SHALL remain Linux and WSL2. A
packaging change SHALL NOT silently change commands, environment names,
artifact names, or client setup. Release artifact names and their SHA-256
hashes are contract; the order of lines in `SHA256SUMS` is not (Q10). The
first Rust release package is version 0.3.0 and carries the Rust catalog;
publishing it, including to npm, stays a separate rollout step (D28).

#### Scenario: Standalone defaults
- **WHEN** an operator starts the agent with the documented standalone defaults
- **THEN** it binds the documented loopback endpoint and keeps mutations denied by default

#### Scenario: Platform defaults
- **WHEN** an enrolled operator starts the platform mode with valid identity and credentials
- **THEN** it uses the documented platform bind and port behavior without changing Platform ownership of intent or routing

### Requirement: Typed target and effect admission

The Host Agent SHALL resolve tenant-scoped canonical resource URIs and resource
kinds before execution, apply the recorded authorization, approval, effect,
and mutation gates as amended by the accepted decisions, and pass
tool-specific arguments to the owning capability without guessing or
rewriting IDs. `vm:` and `container:` targets SHALL remain distinct. No
success result may be invented from a rejected or unknown target. The
standalone mutation gate SHALL follow `standalone-read-only-gate` (decision
D10): with mutations disabled it runs only tools whose effect is `read`.

#### Scenario: Mutation gate remains closed
- **WHEN** a standalone client calls a tool whose effect is not `read` while the explicit mutation gate is disabled
- **THEN** Rust denies the call before side effects with the recorded typed error

#### Scenario: Runtime kind differs
- **WHEN** an operation requires a container but the canonical target resolves to a virtual machine
- **THEN** Rust rejects the mismatch before side effects instead of coercing the URI or choosing another executor

### Requirement: Revisioned provider-neutral catalog and lifecycle

The Host Agent SHALL expose neutral capability descriptors derived from the
active provider generations. Candidate providers SHALL remain isolated until
their manifest, schema, dependency, catalog, and readiness checks pass; failed
activation SHALL leave the prior active generation available. In-flight work
SHALL remain bound to the generation that accepted it, and disposal SHALL be
bounded, idempotent, and reverse-ordered. The catalog revision is an opaque
token that changes whenever the catalog changes; its computation is not
contract (Q12).

#### Scenario: Candidate activation fails
- **WHEN** a candidate provider fails readiness or catalog publication
- **THEN** the active catalog and generation remain usable and Rust reports the failed candidate truthfully

#### Scenario: Active generation changes
- **WHEN** a ready candidate becomes active while an earlier operation is running
- **THEN** the earlier operation retains its original generation and new work receives the new catalog revision

### Requirement: One typed plan and recipe execution path

The Host Agent SHALL keep the recorded versioned recipe validation, canonical
hashing, plan extraction, single generic executor, durable node outcomes,
readiness assertions, bounded retry, cancellation, recovery, and declared
compensation behavior. A provider SHALL NOT create a second workflow runner or
bypass plan admission with a callback.

#### Scenario: Recipe validation
- **WHEN** a client supplies a supported recipe with invalid bindings or a stale catalog revision
- **THEN** Rust rejects it before side effects with the recorded typed validation outcome

#### Scenario: Interrupted plan
- **WHEN** a running plan is cancelled or the process restarts
- **THEN** Rust exposes the truthful durable state and only resumes, compensates, or stops according to the recorded plan contract

### Requirement: Durable and redacted observations

The Host Agent SHALL keep durable operation, task, plan, provider generation,
and observation semantics across restart and cutover. Durable records SHALL
use schema-derived redaction for secrets; unknown projections SHALL fail
closed. Migrating an existing Go installation's local state SHALL preserve
record meaning and provide a verified rollback path before Rust becomes its
writer.

#### Scenario: Secret-bearing input
- **WHEN** a capability accepts a write-only credential field
- **THEN** the value is available only to its authorized transient execution binding and is absent from durable records, client-visible evidence, and logs

#### Scenario: State migration cannot be verified
- **WHEN** an existing Go state copy cannot be migrated and checked without loss or ambiguity
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

### Requirement: Cutover requires specification-based evidence

The Go implementation SHALL remain the production owner until the Rust
implementation passes the Rust-only contract suites, the end-to-end journey
gates on a production-like host, each of which names the requirements it
proves, and the state-conversion and rollback evidence, and until the Host
Agent, Platform, release, and deployment owners have reviewed it (Q13). The
gates are e2e-1 to e2e-4; e2e-5 joins the set once a Platform staging identity
exists and is recorded blocked until then. A missing or blocked gate SHALL be
recorded as unverified or blocked, not converted into a pass. After cutover,
Go SHALL stay installed and selectable for a rollback window of 14 days.
Cutover and Go retirement SHALL be separate, reversible decisions.

#### Scenario: Unit tests pass but journey evidence is absent
- **WHEN** Rust unit and contract suites pass but an end-to-end journey gate or the state migration evidence is missing
- **THEN** the cutover gate remains closed

#### Scenario: Rollback window ends
- **WHEN** Rust has served the full surface through the 14-day rollback window and required evidence remains green
- **THEN** the owner may make a separate decision to retire Go artifacts and registrations
