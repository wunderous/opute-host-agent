# Tasks

- [ ] Add `parent_reservation_id` to `AdmissionRequest` and an `inherited`
      flag to `Reservation` in `crates/host-agent/src/resource.rs`.
- [ ] Thread an optional parent reservation through `RunCtx`
      (`crates/host-agent/src/plan/runner.rs`) so the plan-execution
      dispatch closure in `plan_mcp.rs`/`host_recipe_mcp.rs` can present it
      on each node's `tools::dispatch` call.
- [ ] Teach `Coordinator::admit` to honor a live, owner-matching parent
      reservation: return an inherited copy without evaluating `fits()`,
      mirroring `reservationOwnerCanInherit` (agent id match, task id match
      when the parent has one) and `validateParentReservation` (the durable
      record must still exist and be unexpired).
- [ ] Teach `Coordinator::release` to no-op on an inherited reservation.
- [ ] Replace the release-before-dispatch stopgap in `tools::dispatch`'s
      LIFECYCLE branch (`crates/host-agent/src/tools.rs`) with a
      claim-on-launch lease: the synchronous caller releases on return
      unless the spawned plan-execution thread claims the lease first; only
      the thread's own completion (success, failure, cancellation) releases
      a claimed lease.
- [ ] Add the startup reclaim sweep: after task restoration
      (`crates/host-agent/src/app.rs`), release every durable reservation
      owned by a task whose restored status is now terminal.
- [ ] Confirm `restore_snapshot` (`crates/host-agent/src/tasks.rs`) already
      forces a non-terminal restored status to `failed` (it does --
      `restore_working_task_comes_back_failed`); no change expected here,
      just confirm the reclaim sweep reads the post-restore status.
- [ ] Re-run `host-plan-fault-scripts.restart-mid-plan-reconcile` at
      `--repeat 10` against the Go reference; it must stop failing.
- [ ] Re-run the other four fault scripts (`retry-then-success`,
      `permanent-failure-with-compensation`, `readiness-never-true-timeout`,
      `cancel-mid-node`) to confirm no regression from replacing the
      release-before-dispatch stopgap.
- [ ] `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D
      warnings` clean.
- [ ] Regenerate M6 evidence against the new binary and run
      `make parity-verify-m6` to an actual reported pass.
- [ ] Update `evidence/m6/README.md` and the M6 entry in
      `openspec/changes/reimplement-host-agent-in-rust/milestones.md` to
      drop "the resource-reservation lease around a launched run is not
      ported" once this lands.
