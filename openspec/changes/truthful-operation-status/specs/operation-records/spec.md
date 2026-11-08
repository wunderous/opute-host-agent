## ADDED Requirements

### Requirement: An operation record reports its task's terminal status
The Host Agent SHALL record a finished task's terminal status in its durable
operation record, so that `get_operation` and `tasks/get` agree.

#### Scenario: A completed task read back after a restart
- **WHEN** a task-aware call completes and the agent then restarts
- **THEN** `get_operation` for its id reports `completed`, as `tasks/get` does

#### Scenario: A row Go left working beside a terminal snapshot
- **WHEN** Rust starts on state where Go v0.2.4 stopped with a task in flight, leaving its operation row `working` and its task snapshot `cancelled`
- **THEN** `get_operation` for that id reports `cancelled`, as `tasks/get` does
