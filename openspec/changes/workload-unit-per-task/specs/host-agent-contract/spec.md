## ADDED Requirements

### Requirement: Each task runs in its own workload unit
The Host Agent SHALL name a `run_host_command` workload unit from the
operation and the task that run it, so that two tasks never share a unit,
and SHALL stop that unit when the workload is cancelled or times out, so
that a task reported cancelled has no process left running (decision of
2026-11-08, Q29).

#### Scenario: Two tasks run the same command
- **WHEN** two `run_host_command` tasks with the same command run at the same time
- **THEN** each runs in a unit of its own and both start

#### Scenario: A cancelled workload stops its unit
- **WHEN** a `run_host_command` task is cancelled, by `tasks/cancel` or because the agent stops, while its command runs
- **THEN** the agent stops the task's workload unit before it reports the task cancelled, and the command no longer runs

#### Scenario: The same command after a restart
- **WHEN** a task running a command was cancelled by an agent restart and a new task runs the same command after it
- **THEN** the new task starts and completes normally
