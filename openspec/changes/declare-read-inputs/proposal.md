# Declare the inputs a tool reads in its published schema

Status: decided (owner, 2026-11-07, D28). Owner question Q21
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md.

## Why

e2e-4a found two tools that need an input their published `inputSchema`
does not declare. Rust and Go v0.2.4 both read it; the schema allows it only
because it does not forbid extra properties:

- `restart_host_service` declares only `uri` (required). With just `uri`
  it fails with `Error: serviceName is required` (journey `e2e2.host-ops`).
  Go's `RestartHostServiceArgs` has only `ServiceName`.
- `provision_container` pins a bridge address from `ipv4Address`, which its
  schema does not declare (journey `e2e2.incus`; Go
  `internal/domain/incus/incus_container.go`).

A client that builds calls from the catalog cannot discover either input.

## What Changes

As decided, the catalog declares `serviceName` on
`restart_host_service` (or the tool derives it from `uri`) and
`ipv4Address` on `provision_container`. That changes the published catalog
and its revision, so it is a contract change, not a migration fix.

## Impact

- `crates/host-agent/catalog/source.json` and the catalog revision pins.
- The two journeys then call each tool with declared inputs only.
