# Record of an unadmitted provider callback task

## Why

A provider callback into a task-aware host tool creates a delegated child task. The pinned Go agent does not start that child until the provider polls `tasks/get` for it. It writes the child's `operations` row and task snapshot when the task is created, before resolving the binding or admitting the call (`internal/hostmcp/server.go:1642-1647`). If the agent shuts down before the provider acknowledges the handle, Go cancels the child with "The Host Agent shut down before the callback task handle was acknowledged." and restores it as `cancelled` after a restart.

Owner decision D12 (`refuse-before-operation-record`) requires Rust to resolve the binding and admit a task-aware call before persisting any operation record. An unacknowledged child has not been admitted. Rust therefore writes nothing durable for it, and after a restart `tasks/get` for that child reports `task not found`. D12 was decided for calls refused at binding or admission, not for a call that was never admitted because it never started. Choosing either behaviour is a user-visible policy decision, so the migration does not choose one.

**Decided (owner, 2026-10-12): option A, recorded as D18.** The owner asked for the long-term behaviour: durable operation records hold only admitted work, so a delegated child that never starts leaves nothing behind. The case is a declared divergence in the public suite; it waives no other parity gate.

## What Changes

The owner chooses one of the following:

- **A. Extend D12 to unadmitted delegated children (current Rust behaviour).** A delegated child that never starts (unacknowledged at shutdown, or expired before acknowledgement) leaves no durable row. The live wire is unchanged: the provider receives the same `working` handle and the same cancellation while the agent runs. After a restart the handle is unknown.
- **B. Match Go.** Rust persists the child at creation as Go does. Because the binding has not been resolved at that point, a child later refused at binding or admission would leave a row too. This contradicts D12 for delegated children unless Rust also removes the row when admission refuses it, which Go does not do.

Go is unchanged under either option. Until the owner decides, Rust keeps option A and the differential evidence reports the case as an open divergence, not as parity.

## Capabilities

### New Capabilities

- `unadmitted-callback-task-record`: the durable record of a delegated provider callback task that never started.

### Modified Capabilities

None.

## Impact

The evidence bundle is `evidence/m7/unadmitted-callback-task/`. It holds five Go/Rust observations of the scenario `provider-lifecycle.callback.child-task-unacknowledged-at-shutdown`, kept outside the public parity suite in `tools/parity/open-divergences/`. Its `verify.py` requires exactly these four differences and equality everywhere else. The acknowledged-child case `provider-lifecycle.callback.child-task-acknowledged` stays in the public suite. Its only declared difference is the D12 refused-binding row.
