# A validation-only plan node validates after an applied dependency

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D45.** Goal m20. Applies to 0.4.0 from `rust-0.4.0-rc3`.

## Why

A plan node may have a validation and no action. It waits for an effect
that an earlier node started: an address, a rollout, a public route. When
one of a node's dependencies was applied in the same run, the executor skips
the node's preflight validation so that its action runs even if the effect
looks present. A validation-only node has no action, so it went straight to
`node "<id>" has no action after validation failed`, without reading
anything. Such a node could only pass when every dependency was skipped as
already satisfied, which never happens on a first run.

Goal m20's site recipe hit this on `rust-0.4.0-rc2`. Its `guest-address`
node waits for the new guest's address after the guest node created it, and
the run failed there at once. Go v0.2.4 has the same branch
(`internal/plan/runner.go`, `runNode` and `runForEachItem`), so there is no Go
behaviour to port.

Failing-first test on rc2's code:
`plan::runner::tests::validation_only_node_after_an_applied_dependency_validates`
(`evidence/m20/d45-before.log`).

## What Changes

- An applied dependency still forces a node's action. A node with no action
  evaluates its validation, polling as its `timeoutMs` allows, in `run_node`
  and in a `forEach` item alike.
- Nothing else changes: plan validation, node statuses and outputs, and
  nodes that have an action.

## Options rejected

- **Refuse a validation-only node after a mutating one at validation.** That
  removes the way to wait for an effect an earlier node started. The spec's
  readiness assertions exist for that wait.
- **Fold the wait into the earlier node's validation.** The earlier node's
  output is its action's result, so a later node cannot read the awaited
  value from it. A node may also wait on something no single earlier node
  owns, such as a public route.

## Impact

`plan/runner.rs` (`run_node`, `run_for_each_item`) and one test. No tool,
endpoint, setting or default is added. It is a product change, so the next
candidate is rust-0.4.0-rc3, together with D44. 0.3.0 is unchanged: no 0.3.0
journey writes a validation-only node after an applied one.
