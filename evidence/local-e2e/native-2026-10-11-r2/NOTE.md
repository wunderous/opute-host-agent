# Interrupted cycle (kept as evidence)

This cycle ran on `0a378e6a` after the host-service resolver and shape-driver
fixes. It was stopped during the Go canaries and is **not** a passing set.

- Go/Go ×20: **159/160**. `provider-lifecycle.callback.cancel-bridged-task-refused`
  failed iteration 4. Its `state` query read the bridge's operation row as
  `working` on one Go run and `completed` on the other. `tasks/get` reports
  completion before Go writes the row's final status. This is a race in the
  scenario fixture, not a Rust difference.
- Go/Rust ×5: 160/160.

The fixture was fixed so the durable read waits for the row's terminal
status. The fix changes the harness hash, so every capture was rerun in
[`native-2026-10-11-r3`](../native-2026-10-11-r3/). Nothing here was
relabeled.
