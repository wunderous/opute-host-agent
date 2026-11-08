## ADDED Requirements

### Requirement: An interrupted plan or task reads back truthfully
After a restart, the Host Agent SHALL report every plan run and task the
previous process did not finish with one truthful status in every place it is
read, and SHALL NOT resume, re-run or compensate it implicitly (D28, Q5).

#### Scenario: Graceful stop with work in flight
- **WHEN** the agent is stopped with SIGTERM while a plan runs or a task works, and then starts again
- **THEN** the plan reads back `cancelled`, the task reads back `cancelled` through `tasks/get` and `get_operation`, and neither runs again

#### Scenario: SIGKILL with work in flight
- **WHEN** the agent is killed while a plan runs or a task works, and then starts again
- **THEN** the plan reads back `unknown`, the task reads back `failed` through `tasks/get` and `get_operation`, and neither runs again
