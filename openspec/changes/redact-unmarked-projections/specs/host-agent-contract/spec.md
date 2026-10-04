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
verified by its own contract suite. `oauth-issuance` (decision D8) and
`evidence-redaction` (decision D13) are such divergences.

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
