# Tasks

## 1. Decision record

- [x] 1.1 Record owner decision D14 (2026-10-11) in the migration's
  milestones decision table, with the revision blocking M8. Verify:
  `make spec-validate` passes and the D14 row links this change.
- [x] 1.2 Add D14 to `parity-manifest.json` decisions, with the manifest's
  own provenance rules. Verify: `python3 -m parity manifest` is idempotent,
  and `make parity-verify-m3` output is identical with and without the entry.
  That target already reports stale manifest evidence since the M7 harness
  changes; M7 captures are checked by their own receipts.

## 2. One service context

- [x] 2.1 Generalize `provider/mounts.rs` so a mounted service's
  implementation is either the built-in provider or an MCP adapter, with one
  registration, dependency, family, resolution and disposal path for both,
  and built-in identities under the reserved `host-agent:` prefix. Verify: focused unit
  tests for both kinds, and the 157 public and 55 Go-twin M7 captures rerun
  green over five iterations. Evidence: `builtin.rs` and `mounts.rs` unit
  tests; `evidence/local-e2e/native-2026-10-11` Go/Rust 160/160 ×5 and
  Go/Go 160/160 ×20.
- [x] 2.2 Mount built-in providers from the composition root in dependency
  order, disposed in reverse, with no capability moved yet. Verify: the M1
  lifecycle captures (partial startup, reverse cleanup) rerun green, plus a
  unit test that a failing built-in mount disposes earlier mounts and opens
  no listener. Evidence: `builtin.rs` tests; `app::compose` fails before
  `serve` binds; M1 lifecycle items pass in the final Go/Rust run and the
  `m1` gate.
- [x] 2.3 Add a fake-provider scenario that depends on, and publishes into, a
  capability family a built-in service also implements. Verify: it matches Go
  over five iterations with no new normalization. Evidence: the three
  `provider-lifecycle.builtin-family.*` scenarios in `evidence/seam/m7`.

## 3. Seam verifier

- [x] 3.1 Implement the seam verifier as a Rust test that builds the
  composition root, walks the service context, and cross-checks the
  descriptor source and the captured `tools/list`. Report each capability's
  roles and each M1–M7 gap by name. Verify: it fails when a definition
  without a provider is reached by any executor (a gap), reports one that
  every executor refuses with `not_implemented` as unported, and a provider
  without a definition is refused at mount. Evidence:
  `crates/host-agent/src/seam.rs`, `builtin.rs` tests.
- [x] 3.2 Add a canary that dispatches one tool by name outside the service
  context. Verify: the canary turns the verifier red, and its receipt is
  recorded. Evidence: `evidence/seam/capture.py` records each stage's canary
  under `evidence/seam/<stage>/canary/`; red at every stage.
- [x] 3.3 Commit the gap report as the revision baseline in
  `evidence/seam/`, with a `verify.py` that reproduces its receipt. Verify:
  `python3 evidence/seam/verify.py` reproduces the committed receipt.
  Evidence: `evidence/seam/baseline-validation.json` (28 gaps).

## 4. Revise M1–M7 capabilities (blocks M8)

- [x] 4.1 Move the M3 read-only host tools onto built-in providers, with
  seams limited to what each declares. Verify: the M3 catalog matrix
  (including the revision), the M3 read-only scenarios and the secret sweep
  rerun green; the verifier reports no M3 gap. Evidence:
  `evidence/seam/m3-captures-validation.json`; the M3 secret sweep was
  discarded (contaminated tree) and is covered by the M4 sweep and the final
  run.
- [x] 4.2 Move the M4 admission, resource and task-aware tools onto the seam.
  Verify: the M4 admission matrix, task scenarios, D10 gate suite and D12
  canaries rerun green; the verifier reports no M4 gap. Evidence:
  `evidence/seam/m4-validation.json`, `m4-captures-validation.json`.
- [x] 4.3 Move the M6 recipe and plan tools onto the seam. Verify: M5 and M6
  captures rerun green; the verifier reports no M6 gap. Evidence:
  `evidence/seam/m6-validation.json`, `m6-captures-validation.json`, and the
  final M5 run (crash, cross-read, migration, secrets, unknown projection).
- [x] 4.4 Move the M7 provider lifecycle tools onto the seam. Verify: the M7
  public, Go-twin and real-provider captures rerun green; the verifier
  reports no M7 gap. Evidence: `evidence/seam/m7-validation.json` (0 gaps),
  `m7-captures-validation.json`.
- [x] 4.5 Update the README of each affected milestone's evidence with the
  rerun receipts. Verify: each README links its new receipts, and each
  receipt's `verify.py` reproduces it. Evidence: the D14 sections of
  `evidence/m3`, `m4`, `m6` and `m7` READMEs, and `evidence/seam/README.md`.

## 5. Integration gate

- [x] 5.1 Run the full suite: workspace tests, Clippy `-D warnings`, fmt, the
  locked release build, every milestone's parity verify target, the seam
  verifier and `make spec-validate`. Verify: each passes, or fails
  identically with the pre-seam build and is reported as an existing gap;
  and a committed seam receipt reports no gap. Evidence:
  `evidence/local-e2e/native-2026-10-11/validation.json`. Gates m0–m4 pass.
  The m5, m6 and cutover gates fail on the M5 shape check of 40 M7 provider
  scenarios: the same 200 runs fail with `598d016d`, before any D14 code.
  Cutover also fails on 76 owned inventory gaps. This opens the M8 entry gate. No production
  instance, shared runtime or cutover step is involved.
