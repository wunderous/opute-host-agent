# Reimplement Opute Host Agent in Rust without changing behavior

## Why

The Go Host Agent has accumulated transport, provider, plan, persistence, and
release responsibilities across many packages. A new Rust implementation can
make ownership and lifecycle boundaries easier to maintain, but a language
change is not permission to change what operators or clients can do.

## What Changes

- Build a new Rust Host Agent in `/home/houman/github/opute/host-agent`, using
  the pinned `wunderous/host-agents` commit
  `ace7013df17528fee1bed13a1d70a132d6c5eb9b` as the initial behavioral
  baseline. Rebase the inventory explicitly if Go changes before cutover.
- Replace implementation structure with a smaller set of ownership-focused
  modules, one canonical capability descriptor source, explicit lifecycle and
  dependency wiring, and one plan executor. Preserve existing process and
  protocol boundaries where clients or providers rely on them.
- Build executable, per-surface parity evidence before selecting Rust for any
  production traffic. Keep Go as the production owner and rollback candidate
  until a separately reviewed whole-agent cutover passes.
- Exclude the earlier uncommitted Rust port completely. This change copies no
  code, contracts, or design from it.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This is a pure implementation refactor, so `.openspec.yaml` sets
`skip_specs: true`. The repository's
[Host Agent contract spec](../../specs/host-agent-contract/spec.md) records the
behavior to preserve; this change has no requirement delta.

## Impact

- New local Rust repository and build/release tooling; no remote is configured.
- The Go Host Agent, Platform integration, npm launcher, operator docs, public
  website, provider executables, schemas, and private site deployment stay in
  their current ownership until their cutover work is separately validated.
- Existing local durable state and deployed Host Agent identities require a
  data-preserving migration and rollback rehearsal before traffic moves.

## Non-goals

- No new or removed tool, API, transport, provider, mode, workflow, or user
  promise; no deliberate schema, authorization, error, or default change.
- No new distributed database, orchestration layer, or Platform responsibility
  in the Host Agent.
- No deployment, service restart, infrastructure cleanup, or deletion of the
  Go implementation during this specification change.
