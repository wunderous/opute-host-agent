# Rollback window of five minutes

Status: **accepted (owner, 2026-11-23) as D54**. Replaces D28's 14-day window.

## Why

D28 set the window during which Go stays installed and selectable after a
host's cutover to 14 days. The owner judged that far too long for
development machines, where the agent is replaced often: "That 14 days is
insane for dev machine during development. Make it only 5 minutes".

## What Changes

After a host's cutover, Go stays installed and selectable for 5 minutes;
after that, retiring Go on that host is a separate decision (task 6.4), as
before. Nothing else changes: no code, unit, launcher or release enforces
the window (it is the runbook's policy, packet section 5), and the
reversibility of cutover and retirement stays.

## Impact

- Spec: *Cutover requires specification-based evidence*, the window and its
  scenario.
- Packet section 5 (steps 5, R1, R2), sections 6 and 8; readiness item
  `rollback-window`.
- Rolling back after the window is still what `e2e4.cutover` rehearses (Go
  on Rust's state), but the runbook no longer keeps Go on the host for it.
