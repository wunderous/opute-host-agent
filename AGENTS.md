# Repository guidance

This repository is the new Rust implementation target for Opute Host Agent.
It is planning-only until the OpenSpec change is reviewed. Read
`openspec/specs/host-agent-contract/spec.md`, the active change under
`openspec/changes/reimplement-host-agent-in-rust/`, and
`baseline/source-lock.json` before implementation.

## Authority and scope

- The pinned `wunderous/host-agents` Go tree, its typed contracts, and its
  focused tests define the behavior to preserve. Resolve disagreements with
  runtime `tools/list` and boundary evidence; do not guess from a tool name.
- Implement no new product capability, endpoint, mode, provider, auth path,
  default, or user-visible policy as part of the language migration. An actual
  contract change needs a separate OpenSpec change and owner decision.
- Do not import or adapt the abandoned `wunderous/opute-host-agent-rs` port.
- Opute Platform owns intent, authorization, durable cross-host orchestration,
  routing, and semantic outcomes. Host Agent executes explicit typed work and
  reports observations. Preserve the exact opaque `OPUTE_REMOTE_AGENT_ID`.
- Preserve provider-neutral catalog authority, canonical resource identity,
  fail-closed admission, one plan executor, generation affinity, schema-driven
  redaction, cancellation, durable state, and MCP 2026-07-28 wire behavior.
- Treat public website, npm launcher, release artifacts, and install scripts as
  compatibility surfaces. Their ownership changes only at an explicit cutover.

## Work protocol

1. Pin and inventory the current Go contract before each Rust milestone. A
   newer Go release requires an explicit rebase of the parity inventory.
2. Identify invariant delta before changing a boundary. This change preserves
   the existing Host Agent invariants and proposes a fail-closed parity gate;
   it retires none. Read `.agents/decisions/rust-cutover-parity-gate.json` and
   implement its verifier before a Rust runtime seam is promoted.
3. Keep Rust modules few and ownership-focused. Do not translate Go package
   names one for one or add trait layers without a real boundary.
4. Verify with schema/catalog comparison, wire tests, durable-state migration
   tests, provider and recipe tests, release packaging, and isolated E2E runs.
   HTTP 200 or a green unit suite alone is insufficient.
5. Never mutate shared Incus, Kubernetes, tunnels, or deployment state merely
   to validate a planning artifact. Later live tests require their owning
   typed capabilities and shared-runtime lease.

Run `make spec-validate` for structural OpenSpec validation. Implementation
tasks and evidence gates are in the change's `tasks.md`.
