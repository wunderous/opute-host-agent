# Restart a system service from the production install

Status: decided (owner, 2026-11-07, D28). Owner question Q23
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md.

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

Decided (D28): the tool describes user services only. It restarts an
`opute-` user unit without blocking and reports it `scheduled`; a system
unit is refused before anything runs, with an error that names
`set_host_service_state` (scope `system`). Found while fixing it: the
published output schema allowed only `active` and required `uri`, while
the tool returned `scheduled` without `uri` for every user unit, so every
successful restart failed its own output check (Go v0.2.4 the same). The
schema now allows `scheduled` and the result carries the call's `uri`.

## Impact

- `crates/host-agent/src/hostservice.rs` or the catalog description.
- `e2e2.host-ops` then restarts Incus with the chosen tool.
