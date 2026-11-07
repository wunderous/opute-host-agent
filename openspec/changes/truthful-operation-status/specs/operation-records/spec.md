## ADDED Requirements

### Requirement: An operation record reports its task's terminal status
The Host Agent SHALL record a finished task's terminal status in its durable
operation record, so that `get_operation` and `tasks/get` agree.

#### Scenario: A completed task read back after a restart
- **WHEN** a task-aware call completes and the agent then restarts
- **THEN** `get_operation` for its id reports `completed`, as `tasks/get` does
