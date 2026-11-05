# Reclaim a crashed plan run's orphaned reservation without a task snapshot (proposed)

Status: proposed. Owner question Q15
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented. Current behaviour matches Go v0.2.4.

## Why

e2e-1 journey 7 (`evidence/e2e/runs/e2e-1-r4/e2e1.restart`; first seen in r2) killed the system
agent with SIGKILL while a host plan launched by `run_host_plan` was running. That is the path every tutorial client takes:
the 2026-07-28 client, the legacy client, `pkg/hostagentclient` and the IDE
configuration. After the restart:

- the run is served once, with status `unknown`;
- its `heavy` admission reservation is still in
  `host-resource-coordinator/reservations.json`, owned by the run's id;
- every new heavy call, including resubmitting the same plan, is refused
  with `host_capacity_saturated` on an idle host until the reservation's
  `expiresAt`, 15 minutes after it was taken. This was reproduced by hand
  and still refused after 70 s.

`plan-reservation-inheritance` requires a crashed plan's orphaned reservation
to be reclaimed on restart. Its mechanism is a task snapshot captured
mid-execution, restored as `failed`, whose reservations are then released.
For a plan that precondition never occurs:

- `run_host_plan` is a lifecycle tool. It is routed before the task
  boundary, so it never becomes an MCP task, even when the client declares
  the Tasks extension (e2e-1-r3, retired).
- The run is tracked by an in-memory task keyed by its run id, in Go v0.2.4
  (`plan_run.go`, `CreateWithID`) and in Rust (`plan_mcp.rs`,
  `create_with_id`). That task is persisted only when the run fails or
  resumes, never while it is in flight. After a SIGKILL there is no snapshot
  to restore, so the reclaim has nothing to act on, in Go as in Rust. In
  e2e-1-r4, `tasks/get` on the run id after the restart answers
  `task not found`.
- M6's `host-plan-fault-scripts.restart-mid-plan-reconcile`, the scenario
  that motivated the requirement, ran with
  `OPUTE_HOST_MAX_HEAVY_OPERATIONS=64`. With 64 heavy slots, the leaked
  reservation never refuses the resubmission, which masks the leak.

The requirement's title and its scenario name ("Resubmit after a SIGKILL
during a plan succeeds") promise an outcome that its mechanism cannot
deliver for a plan. Whether the promise or the mechanism is the contract is
an owner decision.

## What Changes

If the owner accepts Q15:

- on restart, after plan runs are restored, the Host Agent releases every
  durable admission reservation whose owner is a plan run this process does
  not execute, meaning a run restored as not running, the same way a
  terminal task's reservation is released today;
- a reservation still owned by a live run, or by a task the restart
  preserves (for example `input_required`), is untouched.

## Impact

- `crates/host-agent/src/app.rs` (the restart reclaim) and `resource.rs`.
- No catalog, wire or CLI change. A failing-first test reproduces journey 7's
  sequence before the fix.
