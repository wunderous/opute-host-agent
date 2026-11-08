## ADDED Requirements

### Requirement: Each task runs in its own workload unit
The Host Agent SHALL run each `run_host_command` task in a workload unit of
its own, and SHALL stop that unit when it reports the task cancelled.

#### Scenario: Two tasks run the same command
- **WHEN** two `run_host_command` tasks with the same command run at the same time
- **THEN** both start, each in its own unit

#### Scenario: Shutdown cancels a working task
- **WHEN** the agent stops while a `run_host_command` task works
- **THEN** the task reads back cancelled and its process no longer runs
