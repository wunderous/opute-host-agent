# Repository guidance

This repository is the new Rust implementation target for Opute Host Agent.
It is planning-only until the OpenSpec change is reviewed. Read
`openspec/specs/host-agent-contract/spec.md`, the active change under
`openspec/changes/reimplement-host-agent-in-rust/`, and
`baseline/source-lock.json` before implementation.

## Authority and scope

- The contract spec (`openspec/specs/`), the accepted OpenSpec changes and
  decisions, and the design docs (this file and
  `openspec/changes/reimplement-host-agent-in-rust/`) define the behavior.
  The frozen v0.2.4 contract record (`baseline/inventory`,
  `baseline/source-lock.json`) settles what the spec words leave to it.
  Since decision D27 (owner, 2026-11-04) Go is not the authority: do not
  capture new Go baselines or compare Rust with Go. A behavior none of these
  sources decides is an owner question
  (`openspec/changes/archive/2026-11-07-rust-spec-authority/questions.md`); never guess it,
  from Go or from a tool name.
- Go remains the production owner until cutover and the migration source for
  existing hosts' state. The committed parity evidence (m0–m12b, d25, m9) is
  the historical record and is not rerun.
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

1. Name the spec requirements a change touches before making it. A gap in
   the spec gets an OpenSpec proposal and an owner question, not a Go
   lookup.
2. Identify invariant delta before changing a boundary. The Host Agent
   invariants are preserved; cutover is gated by the spec-based evidence in
   `.agents/decisions/rust-cutover-parity-gate.json` as amended by D27.
3. Keep Rust modules few and ownership-focused. Do not translate Go package
   names one for one or add trait layers without a real boundary.
   Every capability, built-in or external, has a Service Definition, a
   Service Provider registered in the one service context, and a Consumer
   (owner decision D14, `openspec/changes/three-role-capability-seam/`).
   Built-in and external capabilities follow one architecture and differ
   only in distribution, trust and declared Go-observable contracts. M1–M7
   capabilities are revised onto the seam before M8 starts.
4. Verify with schema/catalog comparison, wire tests, durable-state migration
   tests, provider and recipe tests, release packaging, and isolated E2E runs.
   HTTP 200 or a green unit suite alone is insufficient.
5. Never mutate shared Incus, Kubernetes, tunnels, or deployment state merely
   to validate a planning artifact. Later live tests require their owning
   typed capabilities and shared-runtime lease.

Run `make spec-validate` for structural OpenSpec validation. Implementation
tasks and evidence gates are in the change's `tasks.md`.
