# Port plan-reservation inheritance and restart reclaim

## Why

Validating M6's `restart-mid-plan-reconcile` fault script surfaced a real
architecture gap, not a timing race. `run_host_plan` admits one `heavy`
reservation for a launched plan and its nodes must share it for the plan's
whole lifetime; a brand-new independent reservation per node dispatch is
structurally incompatible with `class_slot_free`'s exclusivity rule (any
held `heavy` reservation blocks every other admission). Go avoids this with
three cooperating pieces that Rust's `admission.rs`/`resource.rs` do not
have at all:

1. **Parent-reservation inheritance.** `resource.WithReservation` carries the
   launcher's reservation through `context.Context`; a nested `Admit` call
   whose request names that reservation as `ParentReservationID` (and whose
   agent/task identity matches, via `reservationOwnerCanInherit`) gets back
   the *same* reservation (`inherited: true`) without touching `fits()` at
   all. `validateParentReservation` rechecks the durable record is still
   live before honoring the inheritance (`internal/resource/service.go`).
2. **Claim-on-launch leasing.** A lifecycle tool's own reservation is wrapped
   in a `reservationLease`. The synchronous caller releases it on return
   (`releaseIfUnclaimed`) unless the asynchronous run claims it first
   (`claimReservationLease`), in which case only the run's own completion
   releases it (`finish`), and a long-running lease is kept alive by renewal
   (`keepAlive`) well inside the reservation TTL
   (`internal/hostmcp/reservation_lease.go`, `internal/hostmcp/server.go`).
3. **Restart reclaim.** A task snapshot restored with a non-terminal status
   (`working`/`running`) is forced to `failed` on restore --
   `"The Host Agent restarted before the task completed."` -- because its
   in-memory continuation is gone (`internal/tasks/registry.go`'s
   `RestoreSnapshot`). Immediately after, `reclaimTerminalTaskReservations`
   releases every durable reservation owned by a now-terminal task
   (`internal/resource/service.go`'s `ReclaimTerminalTaskReservations`,
   called from `internal/hostmcp/server.go`'s startup sequence).

Without these three pieces, Rust either (a) holds the launcher's `heavy`
reservation through the whole run and blocks its own nodes' `normal`-class
dispatches, or (b) releases it early and leaves nothing bounding how many
concurrent plans can mutually admit, and after a crash mid-plan nothing
ever reclaims the orphaned reservation, so a resubmit after restart is
refused with `host_capacity_saturated` where Go succeeds and reconciles.
A prior patch in this session (`crates/host-agent/src/tools.rs`,
merged in [#10](https://github.com/wunderous/opute-host-agent/pull/10))
reordered release-before-dispatch for `run_host_plan`/`run_host_local_recipe`
as a stopgap for the first symptom; it is not architecturally correct and
does not fix `restart-mid-plan-reconcile`, which still fails. This change
replaces that stopgap with the real architecture.

## What Changes

- `AdmissionRequest` gains a `parent_reservation_id` field; `Reservation`
  gains an `inherited` flag.
- A reservation-context value threads through plan execution (`RunCtx` in
  `crates/host-agent/src/plan/runner.rs`, the only place a node dispatch
  closure is built) so a node's `tools::dispatch` call can present the
  plan's own reservation as its parent instead of requesting an independent
  one.
- `Coordinator::admit` honors an inherited parent: given a live, owner-
  matching parent in context, it returns a copy of the parent reservation
  marked `inherited` without evaluating `fits()`; `Coordinator::release`
  no-ops for an inherited reservation, mirroring Go's `Release`/`Admit`.
- The LIFECYCLE branch of `tools::dispatch` (currently the release-before-
  dispatch stopgap) is replaced with a claim-on-launch lease: the
  synchronous caller releases its reservation unless the plan-execution
  thread claims it first; the thread's own completion (success, failure, or
  cancellation) is what finally releases it.
- Task restore (`crates/host-agent/src/tasks.rs`'s `restore_snapshot`) forces
  any non-terminal restored status to `failed`, mirroring Go's existing
  `restore_working_task_comes_back_failed` test (already present and
  passing) -- this piece already matches Go; it's confirmed in scope, not
  newly added.
- A startup reclaim sweep (new, called from `crates/host-agent/src/app.rs`
  after task restoration) releases every durable reservation owned by a
  task whose restored status is now terminal, mirroring
  `reclaimTerminalTaskReservations`.

## Capabilities

### New Capabilities

- `plan-reservation-inheritance`: a plan's nodes share their launcher's
  admission reservation instead of each admitting independently, and a
  crashed plan's orphaned reservation is reclaimed on restart once its task
  is forced terminal.

### Modified Capabilities

- `host-agent-contract`: the M6 exit gate's restart-mid-plan corpus depends
  on this capability; `host-plan-fault-scripts.restart-mid-plan-reconcile`
  is the scenario that exercises it end to end.

## Impact

- `crates/host-agent/src/resource.rs`, `admission.rs`, `tools.rs`,
  `plan/runner.rs`, `plan_mcp.rs`, `host_recipe_mcp.rs`, `tasks.rs`,
  `app.rs`.
- No Go-side change; this closes a Rust parity gap, not a declared
  divergence, so it does not need a numbered `milestones.md` decision --
  the target behavior is Go's existing behavior, byte-for-byte.
- Supersedes the `tools.rs` release-before-dispatch stopgap from
  [#10](https://github.com/wunderous/opute-host-agent/pull/10) for
  `run_host_plan`/`run_host_local_recipe`; the cancellation-abandonment fix
  in the same PR is unrelated and stays.
- Blocks the M6 exit gate: `host-plan-fault-scripts.restart-mid-plan-reconcile`
  cannot reach parity without it.
