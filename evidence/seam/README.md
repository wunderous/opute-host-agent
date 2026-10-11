# Three-role capability seam (decision D14)

Evidence for the OpenSpec change
[`three-role-capability-seam`](../../openspec/changes/three-role-capability-seam/)
and migration task 7.1. Every capability the Rust agent implements now has a
Service Definition from the descriptor source, a Service Provider mounted in
the one service context (`provider/mounts.rs`), and a Consumer. Built-in and
external capabilities share one architecture. They differ only in the
declared distribution, trust and Go-contract rows of the design.

**Result: the seam verifier reports no gap and its canary turns it red.**
28 capabilities are compliant, 0 are gaps and 122 are unported. The
unported ones are M8 work: they are published where Go publishes them and
fail closed with `not_implemented`. The M8 seam gate is open.

Go authority: `wunderous/host-agents` `ace7013d`, tree `ac17cfb2`, reference
binary `71924b62…`. Every stage capture used harness `8bee7abb…` and no new
normalization.

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

The M7-stage report was rebuilt at HEAD with `verify.py m7 --rerun` and
reproduced byte for byte.

## Final integration cycle

[`native-2026-10-11`](../local-e2e/native-2026-10-11/) reran everything on
the committed tree `9878d67f` with a clean worktree before and after.
[`verify.py`](../local-e2e/native-2026-10-11/verify.py) reproduces its
[receipt](../local-e2e/native-2026-10-11/validation.json) and reruns every
gate report byte for byte.

- fmt, Clippy `-D warnings`, 200 workspace tests (seam verifier included),
  locked release build, 141 harness tests, `make spec-validate` (12/12),
  catalog source current, and the Go reference rebuilt identically.
- Go/Go: 160/160 over 20 iterations. Go/Rust: 160/160 over five iterations.
  That covers M1 lifecycle, the M3 catalog matrix and revision, the M4
  admission matrix, M6, the M7 public, Go-twin and real-provider journeys,
  and the `builtin-family` scenarios.
- Go canaries 14/14 and Rust canaries 24/24 caught. Oracles pass for both
  binaries, and their negative controls fail.
- Contracts: D8 24/24, D10 8/8, D11 3/3, D12 3/3, D13 4/4.
- M5: crash 200/200 seeds, cross-read 18/18, migration 6/6, secret sweep
  with 16 fields and 0 failures, unknown projection 0 failures.
- Go owner tests: the 27 M7 tests ×5 and the core packages pass.
- Gates `m0`–`m4` pass.

### What fails, reported as is

- **M5 shape: 585/785 runs identical.** Gates `m5`, `m6` and `cutover`
  therefore fail. All 200 failing runs come from 40 provider-lifecycle and
  provider-real scenarios. These M7 scenarios now enter the shape matrix
  because it takes every manifest item, and they were never shape-checked
  before. All 99 scenarios covered on 2026-10-05 still pass.
  [`shape-preseam`](../local-e2e/native-2026-10-11/m5/shape-preseam/) reran
  the provider scenarios with a Rust build of `598d016d`, before any D14
  code. Exactly the same 200 runs fail. The seam did not cause this. It is
  an existing M7 durable-state gap:
  - Go writes `resource_registry` rows that Rust does not, for example 220
    against 0 after teardown;
  - `supportedTools` order differs in invocation observations;
  - a refused child callback leaves no operation row in Rust;
  - provider timestamps (`activated_at`, `active_at`) have no shape mask.

  Fixing this is outside the seam's scope.
- **`cutover`** also fails on the 76 owned inventory gaps, as expected.

## Corrections during the work

- The first M3 secret sweep was discarded. The sweep builds its Rust fixture
  from the working tree, and that tree already held later edits. From M4 on,
  no source edit happened during a stage, and tree hashes are recorded.
- Rust canary K23 had a stale patch anchor since an earlier M7 change (zero
  matches at `598d016d`). It was re-anchored to the current admission
  refusal block. The stale run is kept as
  [`m4/rust-canaries.stale-k23.json`](m4/rust-canaries.stale-k23.json).
- `builtin-family.require` installs in Go, so its expected tool count is 143.
