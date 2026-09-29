# Opute Host Agent — Rust reimplementation

This local repository is the **new implementation target** for a behavior-preserving
Rust reimplementation of the Go Host Agent. It currently contains only the
contract baseline and an OpenSpec change. It does not contain a runnable agent.

The source baseline is `wunderous/host-agents` commit
`ace7013df17528fee1bed13a1d70a132d6c5eb9b` (tree
`ac17cfb298f789095e7a3af2219837e3490f798a`); see
[source-lock.json](baseline/source-lock.json). The pinned tree and its typed
contracts/tests are the behavioral reference. A release catalog snapshot is
useful evidence, but the catalog exposed by `tools/list` also depends on mode
and active providers. The full parity inventory is an implementation gate.

The earlier, uncommitted `wunderous/opute-host-agent-rs` port is excluded. No
code, contracts, or design from that port are imported into this repository.
The Go Host Agent remains the production implementation until the whole-agent
parity and cutover gates in this specification pass.

## Read the specification

- [Preserved Host Agent contract](openspec/specs/host-agent-contract/spec.md)
- [Rust migration proposal](openspec/changes/reimplement-host-agent-in-rust/proposal.md)
- [Architecture and parity design](openspec/changes/reimplement-host-agent-in-rust/design.md)
- [Ordered implementation gates](openspec/changes/reimplement-host-agent-in-rust/tasks.md)
- [Milestones and E2E validation plan](openspec/changes/reimplement-host-agent-in-rust/milestones.md)
- [Legacy and compatibility inventory](openspec/changes/reimplement-host-agent-in-rust/legacy-inventory.md)
- [Proposed cutover parity invariant](.agents/decisions/rust-cutover-parity-gate.json)

OpenSpec uses `skip_specs: true` for this change because the requested refactor
changes implementation, not product requirements. The main spec records the
existing behavior that the Rust implementation must preserve. OpenSpec's
[spec-driven workflow](https://openspec.dev/docs/schemas/spec-driven) explicitly
supports this form of behavior-preserving refactor.

## Validate the planning artifacts

Node.js 20.19+ is required for the pinned OpenSpec CLI:

```sh
make spec-validate
```

This validates document structure only. It does not prove Rust parity or
authorize a cutover. No Git remote or deployment target is configured here.
