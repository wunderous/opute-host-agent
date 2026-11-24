## ADDED Requirements

### Requirement: Audit rows are kept within an age and a row bound
The Rust Host Agent SHALL keep durable audit rows (`capability_invocations`) for at most `OPUTE_AUDIT_RETENTION_DAYS` days (default 30) and at most `OPUTE_AUDIT_RETENTION_ROWS` rows (default 50000), pruning the oldest rows first, at start and then every 60 seconds off the request path (owner decision D42). A value that is not a positive integer SHALL mean the default. A row whose operation is still `working` SHALL never be pruned.

#### Scenario: Old rows
- **WHEN** an audit row is older than the retention age and its operation is not working
- **THEN** the next prune removes it

#### Scenario: More rows than the bound
- **WHEN** the table holds more rows than the row bound
- **THEN** the next prune removes the oldest rows beyond the bound, except those whose operation is working

#### Scenario: Sustained load
- **WHEN** the agent serves calls continuously with a row bound below the call count
- **THEN** one prune interval after the load the audit holds at most the bound, and the store grows at a small fraction of its unbounded rate

### Requirement: Finished operations follow the audit after their task's TTL
In the same pass, the Rust Host Agent SHALL prune an operation whose status is `completed`, `failed` or `cancelled` when it is older than the retention age or than every kept audit row, no kept audit row names it, and more than one task TTL (one hour) has passed since its last update. An operation that is `working` or `unknown`, or whose id a plan run or an active capability names as its run id, SHALL never be pruned. Plan runs, active capabilities and resource records SHALL not be pruned by this rule.

#### Scenario: A finished task inside its TTL
- **WHEN** a task finished 30 minutes ago and every audit row naming it has been pruned
- **THEN** `tasks/get` and `get_operation` still answer for it

#### Scenario: A plan run's operation
- **WHEN** a finished operation is older than the retention age and a plan run names it
- **THEN** it is kept
