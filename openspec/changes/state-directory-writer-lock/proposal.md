# One writer per instance state directory

## Why

M12 validation 4 and task 4.4 say that two writers on one state directory
must be prevented, and that the lock is tested. The pinned Go agent
(`wunderous/host-agents` `ace7013d`) has no such lock:

- `state.Open` (`internal/state/store.go:104`) and `authz.Open`
  (`internal/authz/store.go:61`) open SQLite in WAL mode with
  `busy_timeout(5000)`. A second process therefore waits for the write lock
  and then writes. It is never refused.
- Go flocks only host-wide coordination files: `admission.lock` and
  `reservations.lock` in `OPUTE_HOST_RESOURCE_LOCK_DIR`, and the Ollama
  request lock. These are shared on purpose by co-resident agents, so they
  must not be used to refuse an agent. Go's own comment calls the state
  directory "instance-private" (`internal/resource/admission.go:25-27`).
  Nothing enforces that.
- M12a observed this on both implementations (`evidence/m12a`, drill
  `twoWriters`). A second agent started with the first one's
  `OPUTE_STANDALONE_STATE_DIR` and `OPUTE_HOST_AGENT_INSTANCE_ROOT` starts
  and serves. Each agent sees only its own in-memory tasks, and a later
  restart sees both agents' persisted tasks. Rust matches Go, so the
  migration preserved this behaviour.

The risk is real at cutover. Go → Rust → Go runs both binaries against one
instance root, and two units (the user unit and the bootstrap unit) or an
operator's manual start can run two agents on one root. Today the M12a
rehearsal keeps one writer only by procedure: each agent is stopped before
the next starts.

Refusing a second agent is new user-visible policy: a new start-up failure,
a new message, and a new file. The language migration must not add it on
its own (AGENTS.md). This change asks the owners to decide it.

## What Changes

The owners choose one of the following. Nothing is implemented until they
decide.

- **A. Lock in Go and Rust (recommended).** Both agents take an exclusive,
  non-blocking `flock` on one lock file in the instance root, before
  opening any store and before binding the port. They hold it for the
  life of the process. A second agent on the same root exits non-zero
  before serving, with one stable stderr line that names the lock file.
  The kernel releases the lock when the process dies, so a crash leaves no
  stale lock. Go changes first, in `wunderous/host-agents`, as a new
  release. Rust then matches that release through the normal parity
  rebase. This is the only option that also protects the mixed
  Go → Rust → Go rollback path, because each side refuses the other.
- **B. Lock in Rust only.** Rust refuses a second agent; Go does not. This
  is a declared, Rust-only difference (like D12). It protects two Rust
  agents from each other, but not a Go agent started on a Rust-owned
  root, so rollback still depends on procedure.
- **C. No lock.** Keep Go's behaviour. Amend M12 validation 4 and task 4.4
  to require one writer by procedure, which the M12a rehearsal and its
  checks already enforce, and close the owned gap as decided.

To decide under A or B:

1. **Lock file name and place.** For example `<instance root>/agent.lock`.
   When `OPUTE_STANDALONE_STATE_DIR` or `OPUTE_HOST_AGENT_SQLITE_ROOT`
   point outside the instance root, decide whether the lock also covers
   those directories, or whether that configuration is refused.
2. **Refusal contract.** The exit code and the exact stderr line. Whether
   systemd should restart a refused unit. Today the units restart on
   failure, and a refused second unit would loop.
3. **Scope.** Standalone and Platform modes. Native Windows, where `flock`
   does not exist and `LockFileEx` would be the equivalent.
4. **Filesystems.** Whether `flock` on network filesystems is out of
   scope, as it is for the existing admission locks.

## Capabilities

### New Capabilities

- `state-directory-writer-lock`: at most one running agent per instance
  state.

### Modified Capabilities

- `host-agent-contract`: under A, a new start-up refusal; under B, a
  declared Rust-only difference; under C, M12 and task 4.4 are amended to
  one writer by procedure.

## Impact

- **Decision:** D25 in the decision table (`milestones.md`), open until
  the owners choose.
- **Under A:**
  - a Go release and a parity rebase of the pinned tree;
  - a Rust change;
  - the M12a `twoWriters` drill changes from "both start" to "the second
    refuses, identically";
  - the install and upgrade journeys gain a refused-second-unit case.
- **Under B:** the Rust change and a declared divergence with per-side
  expectations.
- **Under C:** only documents change.
- **Ownership:** no deployment, release or website ownership changes, and
  no shared state is touched to validate this proposal.
