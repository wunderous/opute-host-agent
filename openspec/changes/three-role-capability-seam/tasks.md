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

- [ ] 2.1 Generalize `provider/mounts.rs` so a mounted service's
  implementation is either the built-in provider or an MCP adapter, with one
  registration, dependency, family, resolution and disposal path for both,
  and built-in identities under the reserved `host-agent:` prefix. Verify: focused unit
  tests for both kinds, and the 157 public and 55 Go-twin M7 captures rerun
  green over five iterations.
- [ ] 2.2 Mount built-in providers from the composition root in dependency
  order, disposed in reverse, with no capability moved yet. Verify: the M1
  lifecycle captures (partial startup, reverse cleanup) rerun green, plus a
  unit test that a failing built-in mount disposes earlier mounts and opens
  no listener.
- [ ] 2.3 Add a fake-provider scenario that depends on, and publishes into, a
  capability family a built-in service also implements. Verify: it matches Go
  over five iterations with no new normalization.

## 3. Seam verifier

- [ ] 3.1 Implement the seam verifier as a Rust test that builds the
  composition root, walks the service context, and cross-checks the
  descriptor source and the captured `tools/list`. Report each capability's
  roles and each M1–M7 gap by name. Verify: it fails on a definition without
  a provider and on a provider without a definition.
- [ ] 3.2 Add a canary that dispatches one tool by name outside the service
  context. Verify: the canary turns the verifier red, and the harness's
  canary suite records it.
- [ ] 3.3 Commit the gap report as the revision baseline in
  `evidence/seam/`, with a `verify.py` that reproduces its receipt. Verify:
  `python3 evidence/seam/verify.py` reproduces the committed receipt.

## 4. Revise M1–M7 capabilities (blocks M8)

- [ ] 4.1 Move the M3 read-only host tools onto built-in providers, with
  seams limited to what each declares. Verify: the M3 catalog matrix
  (including the revision), the M3 read-only scenarios and the secret sweep
  rerun green; the verifier reports no M3 gap.
- [ ] 4.2 Move the M4 admission, resource and task-aware tools onto the seam.
  Verify: the M4 admission matrix, task scenarios, D10 gate suite and D12
  canaries rerun green; the verifier reports no M4 gap.
- [ ] 4.3 Move the M6 recipe and plan tools onto the seam. Verify: M5 and M6
  captures rerun green; the verifier reports no M6 gap.
- [ ] 4.4 Move the M7 provider lifecycle tools onto the seam. Verify: the M7
  public, Go-twin and real-provider captures rerun green; the verifier
  reports no M7 gap.
- [ ] 4.5 Update the README of each affected milestone's evidence with the
  rerun receipts. Verify: each README links its new receipts, and each
  receipt's `verify.py` reproduces it.

## 5. Integration gate

- [ ] 5.1 Run the full suite: workspace tests, Clippy `-D warnings`, fmt, the
  locked release build, every milestone's parity verify target, the seam
  verifier and `make spec-validate`. Verify: all pass and a committed seam
  receipt reports no gap. This opens the M8 entry gate. No production
  instance, shared runtime or cutover step is involved.
