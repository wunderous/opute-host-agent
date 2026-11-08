## 1. Decision

- [x] 1.1 Owner answers Q29. Decided 2026-11-08: adopt both parts.

## 2. Implementation (only after 1.1)

- [x] 2.1 Failing-first test: two identical concurrent commands each start (`two_tasks_with_the_same_command_get_their_own_units`; red with the shared name `opute-host-workload-f12ae0085444375b1ef85953` from the journey).
- [x] 2.2 Failing-first test: a cancelled workload stops its unit (`a_cancelled_workload_stops_its_unit`, `a_completed_workload_is_not_stopped`); shutdown cancels and joins task workers, so the stop runs before the agent exits.
- [ ] 2.3 Rerun e2e4.restart with the same command before and after a restart.
