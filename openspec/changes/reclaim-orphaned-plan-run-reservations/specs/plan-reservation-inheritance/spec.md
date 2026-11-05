## ADDED Requirements

### Requirement: A crashed plan run's orphaned reservation is reclaimed without a task
On restart, the Host Agent SHALL release any durable admission reservation
owned by a plan run that the restarted process does not execute, including a
run launched without the Tasks extension that has no task record, before it
admits the next call.

#### Scenario: Resubmit after a SIGKILL during a plain run_host_plan
- **WHEN** the process is killed while a plan launched by a plain `tools/call run_host_plan` holds a `heavy` reservation, and the restarted process restores that run as not running
- **THEN** the orphaned reservation is released before the next `run_host_plan` call is admitted, and that call is not refused with `host_capacity_saturated`

#### Scenario: A live run's reservation is not touched
- **WHEN** restart reclaim runs and a reservation's owner is still executing
- **THEN** that reservation is left alone
