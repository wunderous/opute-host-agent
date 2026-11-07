## ADDED Requirements

### Requirement: A result the published output schema allows is not refused
The Host Agent SHALL validate a capability's structured result against its
published output schema as that schema defines it, including fields it marks
nullable.

#### Scenario: Diagnose a host without a bridge
- **WHEN** a client calls `diagnose_bridge` on a host whose bridge has never sent a heartbeat
- **THEN** the result reports the bridge offline with a null `lastHeartbeat.at`, not an invalid-result error
