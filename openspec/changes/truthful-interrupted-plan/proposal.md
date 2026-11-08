# An interrupted plan or task reads back truthfully

Status: decided (owner, 2026-11-07, D28). Owner question Q5
([questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).

## Why

Q5 asked what should resume, compensate or stop after a restart. The owner
decided: a restart leaves an interrupted run's truthful durable state, and
nothing is resumed or compensated implicitly.

e2e-4a journey `e2e4.restart` showed one disagreement left: a task killed
mid-flight reads back from `tasks/get` as `failed` ("The Host Agent restarted
before the task completed."), while its operation row says `unknown`, in Go
v0.2.4 as in Rust.

## What Changes

- On start-up, a task restored from a mid-flight snapshot is written back to
  its operation row as `failed`, so `get_operation` and `tasks/get` agree.
- After a graceful stop (SIGTERM, an in-place upgrade) a running plan reads
  back `cancelled` and a working task `cancelled` with "The Host Agent shut
  down before the task completed." After a SIGKILL a running plan reads back
  `unknown` and a working task `failed`. Nothing is re-run.

## Impact

- `crates/host-agent/src/app.rs` (task restore), `tools.rs`.
- `e2e4.restart` asserts these states without a question.
