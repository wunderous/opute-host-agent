## ADDED Requirements

### Requirement: An unadmitted callback task keeps no durable record
The Rust Host Agent SHALL persist no operation row or task snapshot for a delegated provider callback task before that task has been admitted (owner decision D18, extending D12). A child cancelled because the agent shut down before the provider acknowledged its handle SHALL therefore be unknown after a restart. This is a declared difference from the pinned Go agent, which persists the child at creation and restores it as `cancelled`.

#### Scenario: Shutdown before acknowledgement
- **WHEN** a provider callback creates a delegated child task and the agent shuts down before the provider polls `tasks/get` for it
- **THEN** after a restart Rust reports the child as not found and has no operation row for it, where Go restores it as `cancelled`

### Requirement: The difference is declared, isolated and pinned
The case SHALL be in the public parity suite with the declared divergence `D18.unacknowledged-callback-child`, which SHALL drop only the steps that read the child's rows and its restored handle. Expectations SHALL pin each side's exact values for those steps, and every other step SHALL be compared with Go.

#### Scenario: Evidence for the unacknowledged child
- **WHEN** the unacknowledged-at-shutdown callback case runs Go against Rust
- **THEN** Go's child row is `working` before the restart and `unknown` after it with its handle restored `cancelled`, Rust has no child row and answers `task not found`, and the rest of the scenario matches Go
