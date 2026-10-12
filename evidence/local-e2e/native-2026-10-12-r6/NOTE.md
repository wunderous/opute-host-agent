# r6: one Go/Go failure failed every gate (kept, not relabeled)

The cycle ran on the committed tree `b549a207` with a clean worktree before
and after. Go/Rust was 177/177 over five iterations, M5 shape 870/870, and
canaries, contracts, oracles and the go-sdk OAuth client passed. Go/Go was
176/177 over 20 iterations. In iteration 3 of
`provider-lifecycle.callback.no-delegation`, one Go instance's state query
read the bridged task's operation row as `working` while the other read
`completed`. `tasks/get` reports a bridged task complete before Go writes
that row's final status. This is the same race fixed for
`callback.cancel-bridged-task-refused` in r3.

Every callback case's final state query now waits for the latest operation
row to reach a terminal status. The D12 and D18 cases filter out their child
row, so the wait never depends on it. The rerun is `native-2026-10-12-r7`.
