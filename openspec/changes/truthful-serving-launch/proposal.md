# Report a serving launch truthfully (proposed)

Status: proposed. Owner question Q27
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

e2e-4a journey `e2e3.serving` calls `reconcile_serving_assignment` with a
`process` runtime on the dev VM, where the agent runs as a system unit. The
user service supervisor is ready (lingering on, user bus up). The call
answers `started: true`, but no process starts. The launch script's own log
says:

    Failed to connect to bus: No medium found

The launch runs `systemd-run --user` inside `bash -lc`. The agent's
environment has no `XDG_RUNTIME_DIR`, and the user-bus environment the
command runner adds applies only when `systemctl` or `systemd-run` is the
direct command, not a shell. The script then waits 5 s for a main PID and
exits 1, and that exit status is not an error: the command runner returns
non-zero exits as a result, and the assignment ignores it.

Go v0.2.4 behaves the same (`RunAgentShell` through `hostexec.RunCommand`,
`configureCommandEnvironment`). An agent installed as a user unit has
`XDG_RUNTIME_DIR` and does not hit this. No spec says what the call reports
when its supervisor did not start the process.

## What Changes

If the owner chooses it:
- the serving launch runs with the user-bus environment, as the other user
  service operations do;
- a launch that exposes no main PID fails the call with the launch output,
  instead of reporting `started: true`.

Until the owner decides, Rust keeps Go's behaviour.

## Impact

- `crates/host-agent/src/servingops.rs` (`run_agent_shell`, the launch).
- `e2e3.serving` then asserts that the assignment's process answers.
