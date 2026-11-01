# M12b: the soak (M12 validation 5, evidence)

M12b runs validation 5: Go and Rust twins side by side from one golden
under one seeded mixed workload with periodic restarts, judged against
thresholds declared from a Go/Go shakedown before the soak. The owners set
the soak to 15 minutes and capped every validation step at 30 minutes
(2026-11-01). The earlier 72-hour figure is dropped.

M12 formally depends on M11 (staging Platform), which stays the open
dependency. Nothing was published, no tag was pushed, and no shared Incus,
Kubernetes, registry, tunnel or deployment state was touched. The twins are
disposable qemu overlays of the pinned base golden, with no real
credentials and no production identity.

- **Go authority:** `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`),
  `@opute/host-agent@0.2.3`.
- **Go reference:** `.parity/bin/go-reference`, `71924b62…`.
- **Rust candidate:** the x64 release artifact (`dist/host-agent-linux-x64`),
  `e2951200…`, as m12a pinned it.
- **Branch base:** `main` at `40b48383`, after wunderous/opute-host-agent#23.

The binaries and the harness hash are unchanged since m12a, so its suites
are not rerun: the m12b gate is the m12a gate plus `requireSoak`, and it
re-derives everything else from the committed m12a receipts.

**Result:**

- The soak passes all eight checks, re-derived from its raw samples by
  `parity/soak_verify.py`.
- The m12b gate passes, and gates m0–m6, m8a–m8f, m10 and m12a still pass.
- The seam verifier reports no gap, and its dispatch canary is red.
- The cutover gate fails, as expected, on 14 owned inventory gaps and on the
  four M13 cutover evidence files.

## Rounds

| Round | Head | Result | Notes |
| --- | --- | --- | --- |
| Shakedown | `ae7f57ef` | Go/Go and Go/Rust, 10 minutes each; every step exit 0 | Thresholds derived from Go/Go and committed before the soak (commit `5ede22831`); the Go/Rust shakedown passes every check against them |
| Soak 1 | `00d0947b` | Retired (`.parity/retired/m12b-soak-round1`) | Seven of eight checks passed. The down-sample check counted samples inside restart windows; Rust's two fell inside its own restarts. The check was fixed in its own commit ([NOTE.md](NOTE.md#harness-fixes-during-the-milestone)) |
| Soak 2 | `a9cdb3f3` | Every check passes | 15 minutes, seed 1203 |
| Gates | after `ecedb871` | m0–m12b pass; cutover fails as expected | Inventory recaptured with the soak covered |

## Thresholds (declared before the soak)

Derived by rule from the Go/Go shakedown's raw receipts: Rust may differ
from Go by twice the spread two identical Go twins showed, never less than
the stated floors ([soak-thresholds.json](../../tools/parity/soak-thresholds.json)).

| Measure | Bound |
| --- | --- |
| Latency (each operation, call and call plus task wait) | p50 ≤ 2.15× Go, p95 ≤ 2.25×, p99 ≤ 3.0×, each plus 8.1 ms |
| RSS growth (epoch-end slope) | ≤ Go's plus 100,390 KiB/h |
| FD growth | ≤ Go's plus 36.2/h |
| Thread growth | ≤ Go's plus 4/h |
| State size growth | ≤ 2× Go's plus 727,776 B/h |
| Down samples outside restarts | ≤ Go's plus 1 |
| Outcome classes, restarts, journal errors, final state | equal to Go (journal: no more errors) |

## Results (soak 2)

| Check | Go | Rust | Result |
| --- | --- | --- | --- |
| Iterations | 900 | 900 | equal |
| Outcome classes, per iteration | — | — | 0 mismatches |
| Restarts | 7 SIGTERM, 7 SIGKILL, all ready | the same | equal |
| Provider reloads | 30 completed | 30 completed | equal |
| Input-required tasks | 73 answered, 33 cancelled | the same | equal |
| Journal errors | 0 | 0 | pass |
| Down samples outside restarts | 0 (0 total) | 0 (2 total, both inside restarts) | pass |
| RSS peak, epoch-end slope | 41 MiB, 73 MiB/h | 86 MiB, 129 MiB/h | within bound |
| FDs peak, slope | 15, 21/h | 17, 7/h | within bound |
| Latency | within bounds for every operation | | pass |
| Final state | | | 0 diffs after the masks; D13, D19 and 30 reload revision pairs applied |

Within bounds, but worth naming: Rust's RSS is about twice Go's in absolute
terms (the check bounds growth, not size), and its `tools/list` takes about
21.6 ms at p50 against Go's 10.0 ms (bound: 2.15× plus 8.1 ms). Rust's
restarts are faster: ready in about 0.3 s against Go's 2 s.

## Owned gaps (not closed here)

| Gap | Owner | Why it is open |
| --- | --- | --- |
| M11: staging validation identity, Platform-driven journey, client first-success | platform | Needs staging Platform; M12 depends on M11 |
| State-directory writer lock | core | D25 decided a lock in Go and Rust; it waits on the Go release and the parity rebase |
| linux-arm64 clean-machine install | release | In its own session (`claude/arm64-clean-machine`) |
| WSL2 install; `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution` | release, host | Need a WSL2 runner |
| Windows artifacts | release | Need a Windows build |
| M13 cutover evidence (`packageFirstSuccess`, `externalEffects`, `rollbackRehearsal`, `ownerReview`) | release | Reviewed at the cutover decision |
| Stable 0.2.3 publish decision (5.2) | release | Go's release policy refuses the Rust catalog under `0.2.3` |

The soak gap is closed. `parity-verify-cutover` fails, as expected, on the
items above.

CI's parity job fails only because provider binaries are not built in CI.
That is unchanged by M12b.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (phases `shakedown`, `soak`, `gates`) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| Soak: raw ops, samples, events and final state per twin, the summary and the evaluation | [soak/](soak/) |
| Shakedowns | [shakedown-go-go/](shakedown-go-go/), [shakedown-go-rust/](shakedown-go-rust/) |
| Declared thresholds | [soak-thresholds.json](../../tools/parity/soak-thresholds.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [m8d](verify-m8d.json) [m8e](verify-m8e.json) [m8f](verify-m8f.json) [m10](verify-m10.json) [m12a](verify-m12a.json) [m12b](verify-m12b.json) [cutover](verify-cutover.json) |
| D14 seam stage | [report](../seam/m12b/report.json) |
| The soak's design, masks, findings and fixes | [NOTE.md](NOTE.md) |
| Inventory | [cutover](../../baseline/inventory/cutover.json), [gaps](../../baseline/inventory/gaps.json) |
