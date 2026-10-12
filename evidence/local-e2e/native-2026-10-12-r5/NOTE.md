# r5: one Go/Go failure failed every gate (kept, not relabeled)

The cycle ran on the committed tree `83373230` with a clean worktree before
and after. Go/Rust was 177/177 over five iterations, M5 shape 870/870, and
canaries, contracts, oracles and the go-sdk OAuth client passed. Go/Go was
176/177 over 20 iterations. In iteration 2 of `admission.normal-reads.host`,
the pinned Go reference refused one normal read with `host_capacity_saturated`
("declared resource cost exceeds effective host capacity"), while the other
Go instance admitted it. Every gate requires a clean Go/Go, so m0-m6 and
cutover failed.

The admitting server in that scenario was the only admission fixture left
measuring live host capacity under the parallel 20-iteration load. It now
pins CPU and memory capacity, as every other admission scenario does. Its
`startNoSlots` server still refuses by its minimum-available-memory
setting. The scenario then passed Go/Go 20/20 and Go/Rust 5/5 on its own.
The rerun is `native-2026-10-12-r6`.
