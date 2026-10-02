## MODIFIED Requirements

### Requirement: No capability change during the language migration

The Rust Host Agent SHALL preserve every supported public capability and
operator-visible behavior of the pinned Go baseline. It SHALL neither add nor
remove a product capability, endpoint, mode, provider integration, or default
as part of this migration. Fine-grained names, schemas, effects, errors, and
availability are governed by the pinned typed source contracts and a captured
catalog for each supported mode and provider combination, not by a memorized
tool count. The only exceptions SHALL be declared divergences: changes the
owner approved as improvements, each specified by its own OpenSpec
capability, cited by decision id in every parity comparison it affects, and
verified by its own contract suite. `oauth-issuance` (decision D8) is such a
divergence.

#### Scenario: Compare a supported catalog
- **WHEN** Go and Rust start with equivalent configuration and active providers in isolated fixtures
- **THEN** their authenticated catalogs expose compatible names, descriptions, input and output schemas, effects, approval requirements, idempotency declarations, and resource edges at the same contract revision

#### Scenario: Unsupported or new capability appears
- **WHEN** a Rust catalog adds a name or omits a name present in the equivalent Go catalog without a separately approved contract change
- **THEN** the migration parity gate fails and no cutover is authorized

#### Scenario: Undeclared divergence
- **WHEN** a Go-vs-Rust comparison differs on a path that no approved decision declares
- **THEN** the parity gate fails

#### Scenario: Stale divergence
- **WHEN** a declared divergence path no longer differs between Go and Rust
- **THEN** the verifier reports the declaration as stale and the gate fails until it is removed

### Requirement: Compatible authenticated MCP surface

The Host Agent SHALL preserve the pinned Go Streamable HTTP MCP behavior,
including protocol revision `2026-07-28`, authenticated `server/discover`,
`tools/list`, `tools/call`, structured results and errors, task state, and
cancellation. `/health` SHALL remain an open liveness endpoint, while `/mcp`
SHALL reject missing or invalid credentials under the configured auth mode.
The opt-in legacy handshake exception SHALL remain default-off and limited to
the exact method set authorized by Go ADR 0011; it SHALL NOT waive validation
for modern `server/discover` or `tasks/*` methods. OAuth token issuance in the
Rust implementation SHALL follow the `oauth-issuance` capability instead of
the Go baseline.

#### Scenario: Read-only authenticated first call
- **WHEN** a client presents a valid configured credential and performs discovery, tool listing, and `get_host_info {}`
- **THEN** Rust returns the same contract shape and read-only outcome as Go without enabling mutations

#### Scenario: Invalid bearer token
- **WHEN** a client calls `/mcp` with a wrong bearer token
- **THEN** Rust rejects it with the same authentication status and no tool execution

#### Scenario: Legacy exception remains bounded
- **WHEN** legacy handshake mode is explicitly enabled and a request invokes a modern-only method without modern protocol metadata
- **THEN** Rust rejects that request under the same bounded rule as Go

#### Scenario: Issuance follows the Rust contract
- **WHEN** any caller requests a token from the Rust agent without a client secret or an operator-approved authorization
- **THEN** no token is issued, and the `oauth-issuance` contract suite records the outcome
