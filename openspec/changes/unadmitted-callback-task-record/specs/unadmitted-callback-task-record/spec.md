## ADDED Requirements

### Requirement: An unadmitted callback task keeps no durable record pending the owner decision
Until the owner decides this change, the Rust Host Agent SHALL persist no operation row or task snapshot for a delegated provider callback task before that task has been admitted. A child cancelled because the agent shut down before the provider acknowledged its handle SHALL therefore be unknown after a restart. This is a deliberate difference from the pinned Go agent, which persists the child at creation. It requires owner approval before runtime promotion.

#### Scenario: Shutdown before acknowledgement
- **WHEN** a provider callback creates a delegated child task and the agent shuts down before the provider polls `tasks/get` for it
- **THEN** after a restart Rust reports the child as not found and has no operation row for it, where Go restores it as `cancelled`

### Requirement: The difference remains explicit in evidence
Differential evidence SHALL keep this case out of the parity suite and SHALL NOT report it as Go parity. It SHALL record the Go and Rust observations with source and binary provenance, together with a verifier that requires exactly the declared difference.

#### Scenario: Evidence for the unacknowledged child
- **WHEN** the M7 evidence includes the unacknowledged-at-shutdown callback case
- **THEN** it records the Go restored-cancelled child and the Rust unknown child separately and cites this proposal
