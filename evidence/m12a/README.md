# M12a: cutover rehearsals without Platform (evidence)

M12a runs the M12 items that need no staging Platform:

- the store inventory;
- the conversion question;
- forward and rollback rehearsals on checksummed state copies;
- the in-flight drill;
- the two-writer check.

It also reruns every earlier suite against the rebuilt release artifact.

M12 formally depends on M11 (staging Platform), which stays the open
dependency. Nothing was published, no tag was pushed, and no shared Incus,
Kubernetes, registry, tunnel or deployment state was touched. Every phase
ran on isolated copies.

- **Go authority:** `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`),
  `@opute/host-agent@0.2.3`.
- **Rust candidate:** the x64 release artifact (`dist/host-agent-linux-x64`,
  `e2951200…`). It is rebuilt with `make artifacts` after the shutdown drain
  fix.
- **Rust release set:** `SHA256SUMS` `3c7eb0af…`.
- **Go release set:** `7f8d3a89…`.
- **Branch base:** `main` at `bd239c76`, after wunderous/opute-host-agent#21.

**Result:**

- The m12a gate passes, and gates m0–m6, m8a–m8f and m10 still pass.
- The seam verifier reports no gap, and its dispatch canary is red.
- The cutover gate fails, as expected. It fails on 15 owned inventory gaps
  and on the four M12/M13 cutover evidence files.

## Rounds

| Round | Head | Result | Notes |
| --- | --- | --- | --- |
| 1 | `90ad52c1` | Retired (`.parity/retired/m12a-round1`) | 34 of 35 steps exited 0, including every rehearsal check. `dist` failed on one Ubuntu boot-journal comparison: journald recorded the agent's stderr line after systemd's exit lines on Rust's first bootstrap start, with identical content ([NOTE.md](NOTE.md#harness-and-manifest-fixes-during-the-milestone)). |
| 2 | see [checks/head](checks/head) | All 35 steps exited 0. The tree was clean before and after, and the binaries and both release sets were unchanged during the round. | The gate reports first failed on two pins: the manifest's Rust candidate held the host build, and the shape verifier asked for five repeats. Each was fixed in its own commit, and no capture changed. |

## Results

| Place | Go/Go (control) | Go/Rust | What was compared |
| --- | --- | --- | --- |
| Forward rehearsal ×3 | pass | pass, equal to the control | Go stopped with E in flight; a checksummed snapshot; the target on a copy answers A–E over MCP, restarts and answers again, equal to Go's own restart on a copy; snapshot unchanged |
| Rollback rehearsal ×3 | pass | pass, equal to the control | Go on the target's copy reports A–E and the target's new task G with the status and message the target last reported; no refusal and no silent drop; agent ID kept; every stop exits 0 |
| In-flight: synchronous call | drain | drain (after the fix) | At SIGINT: alive while the call runs, new connections refused, 200 with `Connection: close`, exit 0 |
| In-flight: admitted task | drain | drain | At SIGINT: alive until the shim returns, new connections refused, exit 0; after a restart the task is cancelled ("The Host Agent shut down before the task completed.") |
| In-flight: task in binding | Go: cancelled | Rust: unknown task | D12 per-side expectations; the stop itself matches |
| Two agents, one state directory | second starts | second starts | Go takes no lock; neither agent sees the other's in-memory tasks; a restart sees both. The refusal the plan asks for is an owned gap |
| Conversion | — | none needed | Rust reads Go's stopped state as is |
| T1 differential | 327/327 ×3 | 327/327 ×3 | All suites, against the release artifact |
| T2 twins | 34/34 ×3 | 34/34 ×3 | All suites |
| M5 | — | crash 200/200, cross-read 18/18, migration 6/6, shape 972/972, secrets 16 fields (8 D24) | Durable state |
| Distribution (M10 round) | 0 diffs | 0 diffs | Artifacts, 19 log classes, Ubuntu 24.04 and Debian 12 clean machines, Go → Rust → Go, npm 21/21, site |
| Contracts, canaries, oracles | — | 42/42 contract scenarios; Go canaries 14, Rust canaries 24; oracles pass | |

## Owned gaps (not closed here)

| Gap | Owner | Why it is open |
| --- | --- | --- |
| M11: staging validation identity, Platform-driven journey, client first-success | platform | Needs staging Platform; M12 depends on M11 |
| State-directory writer lock | core | Go takes no lock; a refusal is new policy and needs its own OpenSpec change and owner decision |
| 72-hour soak | release | Not run |
| linux-arm64 clean-machine install | release | No aarch64 emulator; the owner chose to keep it owned rather than install one |
| WSL2 install; `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution` | release, host | Need a WSL2 runner |
| Windows artifacts | release | Need a Windows build |
| M13 cutover evidence (`packageFirstSuccess`, `externalEffects`, `rollbackRehearsal`, `ownerReview`) | release | Reviewed at the cutover decision |
| Stable 0.2.3 publish decision (5.2) | release | Go's release policy refuses the Rust catalog under `0.2.3` |

`parity-verify-cutover` fails, as expected, on these items.

CI's parity job fails only because provider binaries are not built in CI.
That is unchanged by M12a.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| Rehearsals and drills | [rehearsal/](rehearsal/) (outcome, results, summary) |
| Distribution round | [dist/](dist/) |
| T1 and T2 suites | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json), [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [m8d](verify-m8d.json) [m8e](verify-m8e.json) [m8f](verify-m8f.json) [m10](verify-m10.json) [m12a](verify-m12a.json) [cutover](verify-cutover.json) |
| Contracts, canaries, oracles, M5 | [contracts/](contracts/), [canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [oracles-t2](oracles-t2.json), [m5/](m5/) |
| D14 seam stage | [report](../seam/m12a/report.json) |
| Go's behavior, the Rust change, findings, masks | [NOTE.md](NOTE.md) |
| Inventory | [cutover](../../baseline/inventory/cutover.json), [gaps](../../baseline/inventory/gaps.json) |
