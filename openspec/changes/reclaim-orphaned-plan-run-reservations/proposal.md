# Reclaim a crashed plan run's orphaned reservation without a task (proposed)

Status: proposed. Owner question Q15
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

e2e-1 journey 7 (`evidence/e2e/runs/e2e-1-r2/e2e1.restart`) killed the system
agent with SIGKILL while a host plan launched by a plain `tools/call
run_host_plan` was running. That is the path every tutorial client takes:
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
to be reclaimed on restart, but its requirement is written for the task
path: a task snapshot restored with a forced-terminal status. A plan launched
without the Tasks extension has no task record, so the requirement does not
reach it, and nothing releases the reservation before it expires.

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
