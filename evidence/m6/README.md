# M6: recipes and the single plan executor

M6 is **assumed passed by owner instruction for the native validation goal
on 2026-10-05**. The owner supplied a 102/102 Go/Rust differential result
with five iterations. This assumption does not turn a failing local run
into passing evidence or authorize production cutover.

The native refresh is recorded in
[the local validation report](../local-e2e/native-2026-10-05/README.md).
An initial local run passed 101/102 scenarios; the restart/reconcile case
failed. Local harness fixes correct a compensation status typo, omitted
resubmission masks and a fixed-sleep kill race. The corrected restart case
passes 20 Go/Go and five Go/Rust repetitions. Its barrier observes the
pinned node state (`pending`, `attempts: 1`) after the preceding node is
`applied`. The fresh full bundles pass all 102 cases at 20 Go/Go and five Go/Rust iterations, and the provenance-bound M6 gate passes. No M6
runtime implementation was changed by this native validation work.

## Current implementation and validation surfaces

- `crates/host-agent/src/plan/` owns the single executor: document validation,
  canonical hashing, dependency ordering, interpolation, bounded retry,
  readiness, compensation and durable recovery.
- `crates/host-agent/src/recipe/` implements the host-local recipe envelope,
  source loading, declared inputs, version validation and host-local
  restrictions. Host-local recipes call the same plan executor.
- `plan_mcp.rs` and `host_recipe_mcp.rs` provide plan/recipe validation,
  execution and run observation, with durable SQLite state and
  schema-derived redaction.
- The launcher's admission reservation is claimed when the plan thread
  starts, inherited by node dispatch through `RunCtx::with_reservation`,
  and released when the worker finishes. Early returns release the
  unclaimed reservation. This wiring is present in `tools.rs`,
  `plan_mcp.rs` and the executor context; the previous assertion that it
  was unported is obsolete.
- The versioned `recipe-corpus` surface has 18 scenarios: six host recipes,
  seven invalid-plan cases and five execution fault scripts. The fault
  scripts cover retry, compensation, readiness timeout, cancellation and
  restart/reconcile. They remain subject to independent outcome checks;
  equal execution errors do not establish parity.
- `make parity-verify-m6` and the manifest's `m6` gate exist. The gate
  requires earlier milestone evidence, contracts, canaries, M5 durable
  validation and the recipe corpus. An owner assumption is not a verifier
  result.

## Remaining migration scope

Provider activation metadata, runtime/tunnel recipe families and provider
lifecycle belong to M7. Provider executables, domain operations, packaging,
Platform integration, rehearsal and cutover remain in their respective
later milestones. No provider gains a second workflow executor.

The old unit-only report predated the M6 corpus and reservation inheritance.
It has been replaced here so it does not misstate the current source or
claim evidence that was not collected locally.

## Three-role seam revision (D14), 2026-10-11

The M6 capabilities now run on the built-in provider in the one service
context. The plan fault scripts, invalid-plan and recipe corpora, and the provider-lifecycle scenarios were rerun against the pinned Go agent with no new
normalization: [`m6-captures-validation.json`](../seam/m6-captures-validation.json) (67/67 Go/Rust, five iterations; secret sweep clean) and the verifier receipt [`m6-validation.json`](../seam/m6-validation.json). The full rerun on the committed tree is in
[`native-2026-10-11-r3`](../local-e2e/native-2026-10-11-r3/), and the seam
evidence index is [`evidence/seam/README.md`](../seam/README.md).
