# One workload unit per task (proposed)

Status: proposed. Owner question Q29
([questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Nothing here is implemented. Current behaviour matches Go v0.2.4.

## Why

e2e-4a run `e2e-4a-r1`, journey `e2e4.restart`: a `run_host_command` task
(`sleep 120`) was cancelled by a SIGTERM restart. A second task with the
same command, started after the restart, completed at once with an error:

    Failed to start transient service unit: Unit opute-host-workload-f12ae0085444375b1ef85953.service was already loaded or has a fragment file.

The workload unit name hashes the operation identity and the argv
(`workloadUnitName`). For a task the operation identity is the tool name,
not the task id, so two tasks running the same command share one unit name
(`tools/dispatch_unassigned.go`, `hostruntime/workload.go`; Rust
`hostcommand.rs`, `hostexec.rs`). The cancelled task's unit also kept
running after the agent stopped, so the task reads `cancelled` while its
process runs on. Go v0.2.4 behaves the same. D28 did not decide this.

## What Changes

If the owner chooses it:
- the workload unit is named from the task id, so concurrent identical
  commands run side by side;
- a task cancelled by shutdown stops its workload unit before the agent
  exits, so `cancelled` means the process stopped.

## Impact

- `crates/host-agent/src/hostcommand.rs`, `hostexec.rs`, the shutdown path.
