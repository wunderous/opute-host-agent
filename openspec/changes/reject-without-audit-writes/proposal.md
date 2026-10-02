# Reject calls without audit writes

## Why

The owner approved decision D11 on 2026-10-02 during local E2E validation:
Rust must reject invalid calls without audit writes. The pinned Go agent writes
a `capability_invocations` record for schema-invalid arguments, even though
the handler never runs. Rust deliberately diverges at this boundary; Go and
the pinned baseline remain unchanged.

## What Changes

- Rejections for missing or invalid MCP credentials, unknown tools, stale
  catalog revisions, undecodable or schema-invalid arguments, and the closed
  standalone mutation gate must leave all durable rows and command traces
  unchanged, including invocation audit records.
- Rejection statuses and wire errors retain Go parity, except independently
  approved D8/D10 behavior.
- Reads must still succeed after these rejections on the same running agent.
- Startup provisioning and accepted calls are outside this decision. OAuth
  issuance auditing remains governed by D8.

## Capabilities

### New Capabilities

- `reject-without-audit-writes`: no execution or durable audit writes for the
  explicitly enumerated MCP rejection paths.

### Modified Capabilities

- `host-agent-contract`: record D11 as a Rust-only exception to durable
  observation parity for rejected calls.

## Impact

The Rust dispatch boundary already rejects these cases without audit writes.
The new contract suite makes that policy explicit and checks state contents,
command traces, wire errors, post-rejection reads, and restart persistence.
A differential scenario preserves error comparison while declaring only the
Go invocation-row count as a D11 difference. No Go deployment, catalog,
accepted-operation auditing, provider, packaging, or release ownership changes.
