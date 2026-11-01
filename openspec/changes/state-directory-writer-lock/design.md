# Design: one writer per instance state (D25, option A)

The owners decided option A on 2026-11-01. Go changes first in
`wunderous/host-agents`; Rust follows the Go release after the parity
rebase. The values marked **proposed** need the owners' confirmation (task
1.2) before the Go change is written.

## Where the lock goes in Go

Go's start-up (`ace7013d`):

```text
cmd/opute-host-agent/main.go   error -> one stderr line, exit 1
  cli -> app.Run(ctx, logger)
    NewRuntime
      config.Load
      validateConfig                 "OPUTE_REMOTE_AGENT_ID is required ..." etc.
      buildHostRuntime               opens the state store (hostmcp: state.Open)
    authz.Open(StandaloneStateDir or InstanceRoot)   authz.sqlite
    transport.NewHTTPServer ... Start                binds the listener
```

The lock is taken in `NewRuntime` after `validateConfig` and before
`buildHostRuntime`. That is the first point where the instance paths are
known, and it comes before any store is opened or the port is bound:

- A misconfigured agent still refuses with its existing message. The
  journal lines that M10 compares for the bootstrap unit do not change.
- A refused second agent opens no store, writes no row and binds no port.
- The lock is released when the `Runtime` closes, or by the kernel when the
  process dies.

## Proposed details

| Item | Proposed | Why |
| --- | --- | --- |
| Lock file | `host-agent.lock` in the instance root (`OPUTE_HOST_AGENT_INSTANCE_ROOT`, default `<user config>/instances/<instance id>`), mode `0600`, created if missing, never deleted | The instance root holds the identity's durable state in both modes; in Platform mode `StandaloneStateDir` can be empty and `authz.sqlite` lives in the instance root |
| Other roots | Also lock `host-agent.lock` in `OPUTE_STANDALONE_STATE_DIR` and `OPUTE_HOST_AGENT_SQLITE_ROOT` when they resolve outside the instance root, in that fixed order | Two instances pointed at one state directory are the same hazard; a fixed order prevents two agents deadlocking on each other's partial locks (the lock is non-blocking anyway) |
| Primitive | Unix: `flock(LOCK_EX\|LOCK_NB)` on an open descriptor held for the process life. Windows: `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK\|LOCKFILE_FAIL_IMMEDIATELY)` | The same primitive Go already uses for `admission.lock`; released by the kernel on crash, so no stale-lock cleanup and no PID files |
| Refusal | Return an error, so Go's `main` prints one stderr line and exits 1. Proposed text: `host agent instance is already in use: <absolute lock path> is locked by another process` | Reuses Go's existing error path and exit code; adds no new exit code |
| systemd units | Unchanged. A refused unit fails, and `Restart=on-failure` retries every 3–5 s until the holder stops, then starts | No unit-file change, so no new artifact; the retry is cheap and ends by itself |
| `check` | Takes the same lock non-blocking and reports the same error when it is held | `check` calls `state.Open`, which creates tables (a write); running it next to a live agent is the same hazard |
| Scope | Standalone and Platform modes; Linux, WSL2 and native Windows | One rule for every configuration |
| Filesystems | Network filesystems are out of scope, as they are for `admission.lock` | `flock` semantics on NFS are not guaranteed |

Not proposed: a blocking wait, a lock timeout, or a takeover flag. Each
would be further policy, and the decided behaviour is to refuse.

## Rust, after the Go release is pinned

Rust's start-up has the same shape (`app.rs`: configuration, validation,
then the stores and the listener). It takes the same lock at the matching
point, with the same file, line and exit code, and its own tests mirror
Go's. Under D14 the lock belongs to the agent runtime itself. It is not a
capability and needs no Service Definition.

## Evidence

The M12a `twoWriters` drill becomes a refusal matrix:

- Go then Go;
- Rust then Rust;
- Go then Rust;
- Rust then Go.

In every case the second agent refuses identically (exit 1, the same line
after masking the sandbox path, no store written), and the first keeps
serving with its tasks intact. A SIGKILL case checks that the next agent
starts. The clean-machine journeys gain the user unit and the bootstrap unit
started on one instance: the second unit fails and retries, and starts once
the first stops. The inventory gap closes only when all of these pass on
both sides.

## Risks

- An operator who today runs two agents on one instance by mistake gets a
  refusal after the upgrade. That is the intended outcome, and the error
  names the lock file.
- A restart loop in `Restart=on-failure` units fills the journal slowly
  while the other agent runs. That is accepted to keep the units
  unchanged; the owners may choose a `RestartPreventExitStatus` instead,
  which would be a unit change.
