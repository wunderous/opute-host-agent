# Refuse a plan node that starts a plan run of its own

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D44.** Goal m20. Applies to 0.4.0 from `rust-0.4.0-rc3`.

## Why

Goal m20 brings a public website up with one `run_host_local_recipe`. While
designing it on `rust-0.4.0-rc2`, a host recipe whose node ran
`run_host_local_recipe` passed `validate_host_local_recipe`, started, and
never finished. The outer run holds the heavy admission slot. The inner run
was refused with `host_capacity_saturated` and retried, and the node stayed
pending until the run was cancelled.

The validators already refuse some of these tools. A bare host plan refuses
a `run_host_plan` action, and a recipe refuses `run_host_plan` and
`run_runtime_recipe`, as Go v0.2.4 does (`internal/recipe/recipe.go`,
`rejectNestedPlanRuns`). Five other tools start a plan run too, and neither
validator names them:

- `run_host_local_recipe`
- `run_tunnel_recipe`
- `opute.provider.install`, `opute.provider.reload` and
  `opute.provider.teardown`, each of which runs the provider's recipe.

Go v0.2.4 has the same gap, so there is no Go behaviour to port.

Failing-first tests on rc2's code:

- `plan::schema::tests::validate_rejects_every_tool_that_starts_a_plan_run`
  failed first on `run_host_plan` as a compensation.
- `recipe::host_recipe::tests::every_tool_that_starts_a_plan_run_is_rejected`
  failed first on `run_host_local_recipe` (`evidence/m20/d44-before.log`).

## What Changes

- One list, `plan::schema::PLAN_RUN_TOOLS`, names the seven tools that start
  a plan run.
- `validate_host_plan` and `run_host_plan` refuse a node whose action,
  compensation or recovery names one. A recipe validator refuses one in
  those places and as a validation. The refusal comes before any side
  effect. The text is unchanged for `run_host_plan`
  (`node "<id>" cannot recursively run a host plan`). For the others it is
  `node "<id>" cannot recursively run <tool>`, and in a recipe
  `recipe node "<id>" cannot recursively run <tool>`.
- Installing a provider stays a host prerequisite. It is a typed call made
  before the recipe that uses the provider's tools, as `evidence/m20/README.md`
  shows.

## Options rejected

- **Let a nested run share its parent's admission slot.** That needs a new
  admission rule for runs started from inside a run, and it would make two
  executors' outcomes depend on each other. The executor's one-run model
  (M6) stays.
- **Mark the tools in the catalog instead of a list.** No catalog field says
  "starts a plan run", and adding one is a catalog contract change. The
  list is the existing rule, made complete.
- **Fail the node at run time.** A plan that cannot finish should be refused
  before it starts, as the existing rule does.

## Impact

`plan/schema.rs` (`validate`, `PLAN_RUN_TOOLS`) and `recipe/host_recipe.rs`
(`reject_nested_plan_runs`, which the runtime and tunnel recipes call too).
No tool, endpoint, setting or default is added. It is a product change, so
the next candidate is rust-0.4.0-rc3. 0.3.0 is unchanged: its packet does not
take contract changes beyond D43, and the deadlock needs a recipe that
installs a provider or runs another recipe, which no 0.3.0 journey writes.
