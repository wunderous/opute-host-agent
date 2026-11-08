# Validate a nullable output field as the published schema means it (proposed)

Status: proposed. Owner question Q22
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

e2e-4a journey `e2e2.host-ops` calls `diagnose_bridge` on a standalone host
with no bridge. The tool builds its result with `lastHeartbeat.at` null, and
its published output schema says `{"type": "string", "nullable": true}`. The
output check ignores `nullable` (an OpenAPI 3.0 keyword, not JSON Schema),
so the call fails closed:

    capability "diagnose_bridge" returned invalid result: structured result does not match output schema: property "lastHeartbeat": property "at": expected string, got <nil>

Go v0.2.4's validator ignores `nullable` too, so Go fails the same way. A
diagnosis tool cannot report "no heartbeat yet", its most likely answer on a
host without a bridge.

## What Changes

The owner chooses one:

- the published schema says what it means in JSON Schema
  (`"type": ["string", "null"]`), a catalog change; or
- the output check honours `nullable` for the catalog's own schemas.

## Impact

- `crates/host-agent/catalog/source.json` or `crates/host-agent/src/schema.rs`.
- `e2e2.host-ops` then expects `bridgeStatus: offline` with a null heartbeat.
