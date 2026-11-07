# Restart a system service from the production install (proposed)

Status: proposed. Owner question Q23
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

`restart_host_service` describes itself as "Restart a host-level service such
as Incus". e2e-4a journey `e2e2.host-ops` calls it for `incus.service` on the
dev VM's production-style install (the system unit runs as `opute`, with
Go's recorded sudoers). It fails:

    Error: Failed to restart incus.service: Interactive authentication required.

The tool runs plain `systemctl restart` as the unprivileged agent, so polkit
refuses. Go v0.2.4 builds the same command (`restartServiceCommand`), while
`set_host_service_state` with `scope: system` uses non-interactive sudo,
which the recorded sudoers allows.

## What Changes

The owner chooses one:

- `restart_host_service` restarts a system service through the same
  non-interactive sudo path as `set_host_service_state`; or
- its description says it restarts only `opute-` user units, and system
  services are restarted with `set_host_service_state`.

## Impact

- `crates/host-agent/src/hostservice.rs` or the catalog description.
- `e2e2.host-ops` then restarts Incus with the chosen tool.
