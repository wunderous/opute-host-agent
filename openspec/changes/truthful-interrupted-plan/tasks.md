## 1. Decision

- [x] 1.1 Owner answers Q5 (D28, 2026-11-07).

## 2. Implementation

- [x] 2.1 Failing-first test: a task snapshotted mid-flight reads back failed through tasks/get and get_operation after a restart (`plan_mcp::tests::a_task_killed_mid_flight_reads_back_failed_everywhere`).
- [x] 2.2 Write the restored task's status back to its operation row at start-up.
- [ ] 2.3 Rerun e2e4.restart with these states as pass conditions.
