# Refuse task-aware calls before recording an operation

## Why

The owner approved decision D12 on 2026-10-02 while M4 (admission, resource
identity, tasks) was being validated: where the Go agent has a gap, Rust may
deviate. The admission matrix found one. For a task-aware call (every mutating
tool, plus the task-only residue), the pinned Go agent creates the task and
persists an `operations` row
(`internal/hostmcp/server.go:1642-1647`, `s.state.Create` and
`s.persistTask`) before `DispatchTool` resolves the canonical resource binding
and admits the call (`internal/hostmcp/server.go:566` and `:573`). A call
refused at admission (missing, malformed, foreign-tenant, wrong-kind or
unknown resource URI; coordinator refusal) still leaves a durable operation
record. That breaks the X2 invariant ("rejected calls have no effects ... no
rows appear in `operations`"). In the M4 matrix, Go leaves 64 rows for 24
tools × 4 binding rows (the other rows of that scenario carry no task).

Go is not consistent here either: `stream_vm_console` already resolves its
binding and admission synchronously, before any task exists
(`internal/hostmcp/server.go:1470-1483`).

## What Changes

- A task-aware call refused at admission (resource binding or the host
  resource coordinator) leaves every durable row unchanged and runs no host
  command beyond the read-only inventory lookups resolution needs.
- The wire behavior keeps Go parity. The call still returns a task handle, and
  `tasks/get` still reaches `completed` with the same `isError` result and
  typed `resource_binding` (or coordinator) error. Only the durable operation
  record differs.
- When durable operation records arrive (M5), Rust persists an operation only
  for a call that passed admission. The contract suite added here holds that
  boundary.

## Capabilities

### New Capabilities

- `refuse-before-operation-record`: no durable operation record for a
  task-aware call refused at admission.

### Modified Capabilities

- `host-agent-contract`: record D12 as a Rust-only exception to durable
  observation parity for task-aware admission refusals.

## Impact

The Rust dispatch boundary already writes no operation row for refused tasks.
The contract suite makes this explicit: it checks state digests, command
traces, the task's wire outcome, post-refusal reads and restart persistence. A
Rust canary that injects an operation row on the refusal path must turn the
contract red. The differential matrix scenarios keep comparing every typed
error and declare only the Go `operations` row count as a D12 difference. On
the Go side, X2 exempts only that table, and only through
`x2GoGaps: {"operations": "D12"}`. Rust is always held to X2 in full.
No Go deployment, catalog, accepted-operation recording, provider, packaging
or release ownership changes.
