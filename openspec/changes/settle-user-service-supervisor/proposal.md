# Settle the user service supervisor on its first ensure

Status: decided (owner, 2026-11-07, D28). Owner question Q20
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md.

## Why

e2e-4a journey `e2e2.host-ops` (run `e2e-4a-r1`) calls
`ensure_host_service_supervisor` on the dev VM, where the agent's user has
no running user manager yet. The first call enables lingering and then fails:

    Error: user service supervisor bus is unavailable: Failed to connect to bus: No such file or directory

A second call a moment later reports `ready`. Go v0.2.4 behaves the same:
it probes the user bus right after enabling lingering, racing systemd's start
of `user@UID`. The T2 host-domain scenario masks the first call's error text
for that reason. No spec says what the first call must return.

## What Changes

As decided, `ensure_host_service_supervisor` waits, within a
bound, for the user bus it has just made persistent before it reports, so
that a single call either reports `ready` or fails with a bounded timeout.
Until the owner decides, Rust keeps Go's behaviour.

## Impact

- `crates/host-agent/src/hostservice.rs` (the supervisor ensure).
- `e2e2.host-ops` then asserts `ready` on the first call.
