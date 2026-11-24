# How this project is built

Opute Host Agent was first written in Go
([`wunderous/host-agents`](https://github.com/wunderous/host-agents)) and then
reimplemented in Rust in this repository, behavior first. The Rust agent has
been the published agent since 0.3.0. The Go tree is frozen and stays
available as a rollback target.

The repository keeps the specification, the decisions and the evidence next to
the code. You don't need any of it to use the agent. It's here for
contributors and for anyone auditing how a release was accepted.

## Layout

| Path | What it holds |
|------|---------------|
| `crates/host-agent/` | The agent: CLI, MCP server, admission, plans, providers |
| `fuzz/` | `cargo-fuzz` targets for the parsers and the wire boundary |
| `openspec/specs/` | The contract the agent keeps ([host-agent-contract](../openspec/specs/host-agent-contract/spec.md)) |
| `openspec/changes/` | Proposed and in-flight contract changes, with their tasks |
| `.agents/decisions/` | Owner decisions and invariants, machine-checked |
| `evidence/` | Per-milestone evidence: gate runs, receipts, end-to-end journeys |
| `baseline/`, `tools/parity/` | The pinned Go baseline and the differential harness used during the migration |

## Build and check

```bash
make rust-check     # rustfmt, clippy -D warnings, unit tests
make rust-build     # target/release/opute-host-agent
make rust-suites    # harness tests and the Rust contract suites
make supply-chain   # cargo-deny: advisories, licences, sources
make fuzz           # bounded fuzz run of every target
make spec-validate  # OpenSpec structure (needs Node.js 20.19+)
```

CI runs the same targets on every pull request, plus a reproducible-build
check that two clean, locked, offline builds are byte-identical.

## How a change is accepted

1. **Contract first.** A change to behavior a client can observe starts as an
   OpenSpec change under `openspec/changes/` with an owner decision. The
   language migration added no capability; new capabilities go through the
   same route.
2. **One capability architecture.** Every capability, built in or external,
   has a service definition, a provider registered in the one service context,
   and a consumer
   ([three-role capability seam](../openspec/changes/three-role-capability-seam/)).
3. **Evidence, not a green unit suite.** A release candidate passes schema and
   catalog checks, wire tests, durable-state migration tests, release
   packaging, and end-to-end journeys on a disposable VM. The journeys install
   the real package, build Incus guests and K3s clusters, and check rollback
   to the previous release.
4. **Readiness.** `evidence/cutover/readiness.py` admits a candidate only when
   every gate passed on that exact build. The review packet for each release
   is in
   [cutover-review-packet.md](../openspec/changes/reimplement-host-agent-in-rust/cutover-review-packet.md).

## History of the migration

- [Migration proposal](../openspec/changes/reimplement-host-agent-in-rust/proposal.md)
  and [design](../openspec/changes/reimplement-host-agent-in-rust/design.md)
- [Milestones and E2E plan](../openspec/changes/reimplement-host-agent-in-rust/milestones.md)
- [Legacy and compatibility inventory](../openspec/changes/reimplement-host-agent-in-rust/legacy-inventory.md)
- [Cutover parity gate](../.agents/decisions/rust-cutover-parity-gate.json)
- [Parity harness](../tools/parity/README.md)

Go parity checks stopped once the Rust agent shipped. The Rust agent is now
built against the specification in `openspec/specs/`.
