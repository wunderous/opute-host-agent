## ADDED Requirements

### Requirement: The Host Agent knows no orchestrator

The Host Agent SHALL execute explicit typed assignments and report observed
outcomes, the same way for every client. Orchestrators, Opute Platform and its
LLM layer included, SHALL retain intent, authorization decisions, cross-host
orchestration, target assignment, routing, and semantic claims about
user-request completion. A successful Host Agent tool call SHALL NOT itself
assert that a natural-language request was satisfied.

#### Scenario: Tool call succeeds
- **WHEN** a Host Agent tool returns a successful typed result
- **THEN** the agent records the result and evidence for whichever client called, without inventing a semantic completion decision

### Requirement: Compatible configuration and distribution

The Host Agent SHALL keep the single serving profile and its documented
defaults (`platform-neutral-host-agent`), the CLI entrypoints and configuration
precedence, and the recorded launcher, installer, and release-artifact
contracts. Native host execution support SHALL remain Linux and WSL2. A
packaging change SHALL NOT silently change commands, environment names,
artifact names, or client setup. Release artifact names and their SHA-256
hashes are contract; the order of lines in `SHA256SUMS` is not (Q10). The
first Rust release package is version 0.3.0 and carries the Rust catalog;
publishing it, including to npm, stays a separate rollout step (D28).

#### Scenario: Profile defaults
- **WHEN** an operator starts the agent with only an agent id and its authentication
- **THEN** it binds the documented loopback endpoint and keeps mutations denied by default

## MODIFIED Requirements

### Requirement: Typed target and effect admission

The Host Agent SHALL resolve tenant-scoped canonical resource URIs and resource
kinds before execution, apply the recorded authorization, approval, effect,
and mutation gates as amended by the accepted decisions, and pass
tool-specific arguments to the owning capability without guessing or
rewriting IDs. `vm:` and `container:` targets SHALL remain distinct. No
success result may be invented from a rejected or unknown target. The
mutation gate SHALL follow `standalone-read-only-gate` (decision D10), keyed
on `OPUTE_HOST_ALLOW_MUTATIONS`: with mutations disabled it runs only tools
whose effect is `read`.

#### Scenario: Mutation gate remains closed
- **WHEN** a client calls a tool whose effect is not `read` while `OPUTE_HOST_ALLOW_MUTATIONS` is not `true`
- **THEN** Rust denies the call before side effects with the recorded typed error

#### Scenario: Runtime kind differs
- **WHEN** an operation requires a container but the canonical target resolves to a virtual machine
- **THEN** Rust rejects the mismatch before side effects instead of coercing the URI or choosing another executor

### Requirement: Cutover requires specification-based evidence

The Go implementation SHALL remain the production owner until the Rust
implementation passes the Rust-only contract suites, the end-to-end journey
gates on a production-like host, each of which names the requirements it
proves, and the state-conversion and rollback evidence, and until the Host
Agent, Platform, release, and deployment owners have reviewed it (Q13). The
gates are e2e-1 to e2e-4; e2e-5, an external orchestrator client on a host
configured by explicit settings, run in Platform staging, joins the set once a
Platform staging identity exists and is recorded blocked until then. A missing or blocked gate SHALL be
recorded as unverified or blocked, not converted into a pass. After cutover,
Go SHALL stay installed and selectable for a rollback window of 14 days.
Cutover and Go retirement SHALL be separate, reversible decisions.

#### Scenario: Unit tests pass but journey evidence is absent
- **WHEN** Rust unit and contract suites pass but an end-to-end journey gate or the state migration evidence is missing
- **THEN** the cutover gate remains closed

#### Scenario: Rollback window ends
- **WHEN** Rust has served the full surface through the 14-day rollback window and required evidence remains green
- **THEN** the owner may make a separate decision to retire Go artifacts and registrations

## REMOVED Requirements

### Requirement: Compatible modes, configuration, and distribution

**Reason**: The two serving modes become one profile (`platform-neutral-host-agent`).
**Migration**: Replaced by "Compatible configuration and distribution"; hosts set the explicit settings listed in the proposal's migration.

### Requirement: Host Agent and Platform ownership stays separate

**Reason**: The agent no longer names an orchestrator.
**Migration**: Replaced by "The Host Agent knows no orchestrator", which keeps the same ownership split for every client.
