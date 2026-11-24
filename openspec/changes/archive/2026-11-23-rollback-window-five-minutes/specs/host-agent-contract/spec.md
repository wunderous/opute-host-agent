## MODIFIED Requirements

### Requirement: Cutover requires specification-based evidence

The Go implementation SHALL remain the production owner until the Rust
implementation passes the Rust-only contract suites, the end-to-end journey
gates on a production-like host, each of which names the requirements it
proves, and the state-conversion and rollback evidence, and until the Host
Agent, Platform, release, and deployment owners have reviewed it (Q13). The
gates are e2e-1 to e2e-4; e2e-5 joins the set once a Platform staging identity
exists and is recorded blocked until then. A missing or blocked gate SHALL be
recorded as unverified or blocked, not converted into a pass. After cutover,
Go SHALL stay installed and selectable for a rollback window of 5 minutes
(D54).
Cutover and Go retirement SHALL be separate, reversible decisions.

#### Scenario: Unit tests pass but journey evidence is absent
- **WHEN** Rust unit and contract suites pass but an end-to-end journey gate or the state migration evidence is missing
- **THEN** the cutover gate remains closed

#### Scenario: Rollback window ends
- **WHEN** Rust has served the full surface through the 5-minute rollback window and required evidence remains green
- **THEN** the owner may make a separate decision to retire Go artifacts and registrations
