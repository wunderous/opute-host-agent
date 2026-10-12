# r4 attempt stopped by a full /tmp (kept, not relabeled)

This cycle ran on the committed tree `12396739`. `/tmp` (a 16 GB tmpfs) was
already about 11 GB full of per-scenario sandboxes left by earlier harness
runs. Go/Go iterations 0 to 11 passed. From iteration 12 on, every
scenario failed with `OSError: [Errno 28] No space left on device` while
creating its sandbox, which gave 0/177. Every later step failed the same way:
Go/Rust 164/177, shape 860/870, the Go and Rust canary builds, the Go owner
tests and the go-sdk client build. None of these results says anything
about parity.

The leftover sandboxes were removed, and the cycle script now sweeps the
harness's `/tmp/parity-a-*` and `/tmp/parity-b-*` sandboxes between its
sequential phases. The rerun is `native-2026-10-12-r4`.

Only this note, the check logs and the summaries are kept. The raw
per-scenario captures were removed; they record disk-full failures, not
behaviour.
