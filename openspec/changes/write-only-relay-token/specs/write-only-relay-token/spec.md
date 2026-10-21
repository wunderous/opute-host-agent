## ADDED Requirements

### Requirement: The PostgreSQL relay token is published write-only
The Rust Host Agent SHALL publish `relayToken` with `writeOnly: true` in `release_postgresql_service_relay`, and `localRelay.relayToken` with `writeOnly: true` in `get_postgresql_service_status`, `reconcile_postgresql_service` and `remove_postgresql_service`, in every serving mode that publishes the tool (owner decision D24). No other schema keyword, tool name, effect, admission class, result or behaviour SHALL change.

#### Scenario: The catalog marks the token
- **WHEN** a client lists tools in any serving mode
- **THEN** each listed path carries `writeOnly: true` and keeps Go's type and `minLength`

### Requirement: No durable sink keeps the relay token
A relay token sent to any of the four tools SHALL NOT appear verbatim in any durable row, state file, log or task response. The invocation audit, the task's argument echo, plan documents and capability bindings SHALL carry the redaction marker in its place.

#### Scenario: A canary token in each marked field
- **WHEN** the M5 secret sweep sends a canary at each D24 path in both serving modes
- **THEN** every durable projection redacts it and no sink holds the canary

### Requirement: The difference stays declared and exact
`D19.neutral-host-tool-names` SHALL remove exactly the D24 marks from Rust's published schemas before comparison with Go. Any other difference SHALL remain a diff.

#### Scenario: The catalog compared with Go
- **WHEN** a scenario observing `tools/list` runs Go against Rust
- **THEN** the divergence applies, and the observations are identical
