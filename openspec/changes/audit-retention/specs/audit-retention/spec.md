## ADDED Requirements

### Requirement: Audit rows are kept within an age and a row bound
The Rust Host Agent SHALL keep durable audit rows (`capability_invocations`) for at most `OPUTE_AUDIT_RETENTION_DAYS` days (default 30) and at most `OPUTE_AUDIT_RETENTION_ROWS` rows (default 50000), pruning the oldest rows first, at start and then every 60 seconds off the request path (owner decision D42). A value that is not a positive integer SHALL mean the default. A row whose operation is still `working` SHALL never be pruned. No other durable record SHALL be pruned by this rule.

#### Scenario: Old rows
- **WHEN** an audit row is older than the retention age and its operation is not working
- **THEN** the next prune removes it

#### Scenario: More rows than the bound
- **WHEN** the table holds more rows than the row bound
- **THEN** the next prune removes the oldest rows beyond the bound, except those whose operation is working

#### Scenario: Sustained load
- **WHEN** the agent serves calls continuously with a row bound below the call count
- **THEN** the state file stops growing once the bound is reached
