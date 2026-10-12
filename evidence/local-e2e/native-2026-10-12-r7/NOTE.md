# r7: one Go/Go failure failed every gate (kept, not relabeled)

The cycle ran on the committed tree `695e7a99` with a clean worktree before
and after. Go/Rust was 177/177 over five iterations, M5 shape 870/870, and
canaries, contracts, oracles and the go-sdk OAuth client passed. Go/Go was
176/177 over 20 iterations, so gates m0-m6 failed. The cutover gate failed as
expected: 38 M8/M10 inventory gaps and no reviewed cutover evidence.

In iteration 13 of `provider-lifecycle.replacement.in-flight`, one Go
instance's `install2` answer named `com.opute.fixture-2` as the active
runtime's generation, while the other named `com.opute.fixture-1`. `install2`
reuses run 1 through its idempotency key. Go answers with that record
(`plan_run.go`, the found-completed branch) and re-runs the plan in a
goroutine, whose activation validation rewrites the `active_runtimes` row.
The answer reads that row, so it races the goroutine. Under the cycle's
parallel load the goroutine won once. Rust has the same shape
(`plan_mcp.rs`): it answers first and runs the plan on a spawned thread.

The scenario now holds provider2's second `get_install_manifest` reply, the
background validation read, until `install2` has answered. Both hosts then
answer with generation 1 and report generation 2 after the wait. The case
passes Go/Go 20/20 and Go/Rust 20/20 with the gate reached on every side.
The rerun is `native-2026-10-12-r8`.
