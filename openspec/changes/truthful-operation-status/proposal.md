# A durable operation's status for a finished task (proposed)

Status: proposed. Owner question Q25
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

`get_operation` and `list_operations` (ported in e2e-4a) read the
`operations` row's `status` column. For a task-aware call that column is
never the task's real status:

- Go's `SaveTaskSnapshot` takes the status from the snapshot, but the live
  Tasks projection carries a named string type, so the assertion fails and
  every row is written `working`, even for a completed or failed task. Rust
  keeps that row projection on purpose (`store.rs`,
  `save_registry_task_snapshot`);
- after a restart, a `working` row is marked `unknown`.

e2e-4a journey `e2e4.restart` shows it: a task that completed reads back
`unknown` through `get_operation`, while `tasks/get` for the same id says
`completed`, and the row's own `task_snapshot_json` holds the truth. The
contract spec asks Rust to "expose the truthful durable state".

## What Changes

If the owner chooses it, the row's `status` is the task's terminal status
(`completed`, `failed` or `cancelled`), so `get_operation` agrees with
`tasks/get`. Go rolled back onto such a row reads the same column, so the
rollback reading stays compatible. This changes a durable value Go writes
differently, so it needs a decision.

## Impact

- `crates/host-agent/src/store.rs` (`save_registry_task_snapshot`).
- `e2e4.restart` and `e2e4.cutover` then assert the terminal status.
