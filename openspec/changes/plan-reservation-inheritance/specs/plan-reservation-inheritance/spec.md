## ADDED Requirements

### Requirement: Plan nodes inherit their launcher's admission reservation
When `run_host_plan` or `run_host_local_recipe` admits a reservation to
launch a plan, every node the plan dispatches SHALL present that same
reservation as its parent instead of requesting an independent one, for as
long as the parent reservation is live and owned by the same agent (and
task, once bound). Admission for such a nested request SHALL return the
parent reservation unchanged (not a new one, not a capacity decision)
rather than evaluating normal capacity rules against it.

#### Scenario: A node dispatch does not contend with its own plan's hold
- **WHEN** a plan holding a `heavy` reservation dispatches a node whose
  declared cost is `normal` class
- **THEN** the node's dispatch is admitted by inheriting the plan's
  reservation, not refused with `host_capacity_saturated` by the
  class-exclusivity rule

#### Scenario: Inheritance requires a live, matching parent
- **WHEN** a nested dispatch names a parent reservation that has expired,
  been released, or belongs to a different agent or task
- **THEN** admission SHALL refuse the request rather than silently
  inheriting a reservation it cannot prove is still held

### Requirement: A launched plan's reservation outlives the launching call
`run_host_plan`/`run_host_local_recipe`'s own reservation SHALL be released
by whichever of the synchronous launching call or the asynchronous plan
execution finishes last to touch it, and by exactly one of the two: the
synchronous call releases it on return only if the plan execution never
claims it; once claimed, only the plan execution's own completion
(success, failure, or cancellation) releases it.

#### Scenario: A plan outliving its launching call keeps its reservation
- **WHEN** `run_host_plan` returns a `running` status while its plan
  continues executing on another thread
- **THEN** the reservation remains held for the plan's nodes to inherit
  until the plan itself reaches a terminal state

#### Scenario: A plan that fails before claiming the lease still releases it
- **WHEN** `run_host_plan`'s synchronous handler returns an error before
  spawning any execution thread
- **THEN** the reservation is released once, by the synchronous caller

### Requirement: A crashed plan's orphaned reservation is reclaimed on restart
On restart, after a task snapshot captured mid-execution is restored with a
forced-terminal status, the Host Agent SHALL release any durable admission
reservation owned by that now-terminal task, so a resubmission is not
refused by a reservation nothing will ever release.

#### Scenario: Resubmit after a SIGKILL during a plan succeeds
- **WHEN** the process is killed while a plan's `heavy` reservation is held,
  and the restarted process restores that plan's task as `failed`
- **THEN** the orphaned reservation is reclaimed before the next
  `run_host_plan` call is admitted, and that call is not refused with
  `host_capacity_saturated`

#### Scenario: A still-genuinely-live reservation is not touched
- **WHEN** restart reclaim runs and a restored task's status is not
  terminal (for example, `input_required`, which Go and Rust both preserve
  across restart rather than failing)
- **THEN** that task's reservation, if any, is left alone
