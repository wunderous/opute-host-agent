# Three-role capability seam (decision D14)

Evidence for the OpenSpec change
[`three-role-capability-seam`](../../openspec/changes/three-role-capability-seam/)
and migration task 7.1. Every capability the Rust agent implements now has a
Service Definition from the descriptor source, a Service Provider mounted in
the one service context (`provider/mounts.rs`), and a Consumer. Built-in and
external capabilities share one architecture. They differ only in the
declared distribution, trust and Go-contract rows of the design.

**Result: the seam verifier reports no gap, its canary turns it red, and
every milestone gate `m0`–`m6` passes, so the M8 seam gate is open.**
28 capabilities are compliant, 0 are gaps and 122 are unported. The
unported ones are M8 work: they are published where Go publishes them and
fail closed with `not_implemented`. `cutover` fails only on the 76 owned
inventory gaps, as expected.

Go authority: `wunderous/host-agents` `ace7013d`, tree `ac17cfb2`, reference
binary `71924b62…`. The stage captures used harness `8bee7abb…` with no new
normalization. Closing the M5 gate needed one Rust fix and shape-driver
changes, described under "M7 durable-state gap" below.

## How the verifier works

[`seam.rs`](../../crates/host-agent/src/seam.rs) is a Rust test over the real
composition root (`app::compose`) in platform and standalone modes. It
classifies every descriptor-source name as `compliant`, `unported` or `gap`,
and cross-checks the published names against Go's captured `tools/list`.
The computed gap list must equal
[`seam-gaps.json`](../../crates/host-agent/seam-gaps.json), which is now `[]`.

[`capture.py <stage>`](capture.py) writes the verifier report and runs the
canary. The canary copies the workspace and adds a `list_operations` name
match in the dispatch executor. The verifier must then fail and name the
tool. [`verify.py <stage> [--rerun]`](verify.py) reproduces the report and
canary receipt; `--rerun` rebuilds the report at the current tree and
compares it byte for byte. [`verify_captures.py <stage>`](verify_captures.py)
recomputes each stage's parity captures from raw I/O.

## Stages

Each revision step was captured on its working tree before its commit. The
stage records the parent `head`, the binaries, and tree-diff hashes before
and after, which must be equal: no tracked file changed during a capture.

| Stage | Commit | Verifier receipt | Capture receipt | What reran (Go/Rust, five iterations) |
| --- | --- | --- | --- | --- |
| Baseline | `9d952f08` | [baseline](baseline-validation.json): 28 gaps, 122 unported, canary red | — | Built-in provider mounted, no capability moved |
| M3 | `053cdf5c` | — (committed with M4) | [m3](m3-captures-validation.json) | Catalog matrix with revision, 22 read-only host scenarios (26/26). Secret sweep discarded, see below |
| M4 | `053cdf5c` | [m4](m4-validation.json): 12 compliant, 16 gaps | [m4](m4-captures-validation.json) | M3 set plus the admission matrix, rejection and tasks (44/44); secret sweep 16 fields, 0 failures; D10 8/8, D11 3/3, D12 3/3; Rust canaries 24/24 |
| M6 | `f23e6d75` | [m6](m6-validation.json): 23 compliant, 5 gaps | [m6](m6-captures-validation.json) | Plan fault scripts, invalid-plan and recipe corpora, the 49 provider-lifecycle scenarios (67/67); secret sweep clean |
| M7 | `9878d67f` | [m7](m7-validation.json): 28 compliant, **0 gaps** | [m7](m7-captures-validation.json) | 52 provider-lifecycle (including the 3 `builtin-family` scenarios) and 6 real-provider journeys (58/58), plus Go/Go 58/58; secret sweep clean |
| Final | `5d439346` | [final](final-validation.json): 28 compliant, 0 gaps | the r3 cycle below | Everything, after the host-service resolver joined the providers service's declared `host-observation` dependency |

The M7-stage report was rebuilt at HEAD with `verify.py m7 --rerun` and
reproduced byte for byte.

## Final integration cycle

[`native-2026-10-11-r3`](../local-e2e/native-2026-10-11-r3/) reran
everything on the committed tree `5d439346` with a clean worktree before and
after. [`verify.py`](../local-e2e/native-2026-10-11-r3/verify.py)
reproduces its [receipt](../local-e2e/native-2026-10-11-r3/validation.json)
and reruns every gate report byte for byte.

- fmt, Clippy `-D warnings`, 202 workspace tests (seam verifier included),
  locked release build, harness tests, `make spec-validate` (12/12),
  catalog source current, and the Go reference rebuilt identically.
- Go/Go: 160/160 over 20 iterations. Go/Rust: 160/160 over five iterations.
  That covers M1 lifecycle, the M3 catalog matrix and revision, the M4
  admission matrix, M6, the M7 public, Go-twin and real-provider journeys,
  and the `builtin-family` scenarios.
- Go canaries 14/14 and Rust canaries 24/24 caught. Oracles pass for both
  binaries, and their negative controls fail.
- Contracts: D8 24/24, D10 8/8, D11 3/3, D12 3/3, D13 4/4.
- M5: shape **785/785**, crash 200/200 seeds, cross-read 18/18, migration
  6/6, secret sweep with 16 fields and 0 failures, unknown projection 0
  failures.
- Go owner tests: the 27 M7 tests ×5 and the core packages pass.
- Gates `m0`–`m6` pass. `cutover` fails only on 76 owned inventory gaps.

### Earlier cycles, kept as they are

- [`native-2026-10-11`](../local-e2e/native-2026-10-11/), on `9878d67f`:
  M5 shape 585/785, so `m5`, `m6` and `cutover` failed. All 200 failing
  runs came from 40 M7 provider scenarios, shape-checked for the first time.
  A Rust build of `598d016d`, before any D14 code, failed exactly the same
  runs ([`shape-preseam`](../local-e2e/native-2026-10-11/m5/shape-preseam/)).
- [`native-2026-10-11-r2`](../local-e2e/native-2026-10-11-r2/), on
  `0a378e6a`: Go/Go 159/160, from a fixture race in
  `callback.cancel-bridged-task-refused`. Stopped; see its `NOTE.md`.

### M7 durable-state gap

The owner widened the goal to close it (2026-10-11).

- **A Rust fix (`0a378e6a`).** Rust had stubbed Go's
  `resolveProviderHostService`. A teardown without a `serviceName` therefore
  never resolved the provider's unit. It also never registered the units
  that Go's `ListHostServices` registers along the way: 220
  `resource_registry` rows in Go, 0 in Rust. Rust now ports the resolver
  through the service context, on the providers service's declared
  `host-observation` dependency.
- **Shape-driver changes (`shape.py`, which is not an observing module).**
  The driver applied wall-clock masks only to pre-M7 tables. It now:
  - masks `active_at` (only when set) and `activated_at`;
  - masks restart-recovered `updated_at` with a typed `wall-clock` mask
    that accepts RFC 3339 or SQLite datetime only;
  - applies the F-2 `supportedTools` set when a host tool is reached
    through a callback;
  - projects the scenario's reviewed task masks onto persisted operation
    results;
  - excludes audit rows of a tool that is called only through `until`
    polls (13 callback scenarios' `get_host_capacity` waits);
  - recognises the approved D12 refused callback child by the handle the
    callback reported.

  The M5 gate verifier (`m5_verify.py`) accepts that child under the same
  typed-refusal conditions. The tests in `tests/test_shape.py` show that
  each rule still exposes a real difference.
- **Owner approval (2026-10-11).** The r3 gate first failed m5 and m6 on
  five `callback.child-task-acknowledged` runs, which the M5 gate verifier
  reported as "D12 hides an admitted or unidentified operation row".
  `m5_verify.py` was aligned with the shape driver only after that failure
  (`056361f8`). The owner was asked explicitly and approved, as the M5 shape
  normalization, every shape-driver rule above (not only the timestamp masks
  first approved) and that verifier alignment, as the exit path for task
  5.1. The failing gate reports are kept: rerunning `parity verify` on the
  same r3 evidence with the pre-alignment checker (`0a378e6a`) reproduces
  them as
  [`verify-m5.pre-alignment.json`](../local-e2e/native-2026-10-11-r3/verify-m5.pre-alignment.json)
  and
  [`verify-m6.pre-alignment.json`](../local-e2e/native-2026-10-11-r3/verify-m6.pre-alignment.json),
  each with exactly those five problems. Their logs are in `checks/`.
- **A fixture fix (`5d439346`).** `callback.cancel-bridged-task-refused` now
  waits for the bridged operation's final durable status. Go's `tasks/get`
  reports completion before that row is written. This changed the harness
  hash, so r3 reran everything.

## Corrections during the work

- The first M3 secret sweep was discarded. The sweep builds its Rust fixture
  from the working tree, and that tree already held later edits. From M4 on,
  no source edit happened during a stage, and tree hashes are recorded.
- Rust canary K23 had a stale patch anchor since an earlier M7 change (zero
  matches at `598d016d`). It was re-anchored to the current admission
  refusal block. The stale run is kept as
  [`m4/rust-canaries.stale-k23.json`](m4/rust-canaries.stale-k23.json).
- `builtin-family.require` installs in Go, so its expected tool count is 143.
