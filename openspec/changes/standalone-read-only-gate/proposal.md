# Standalone read-only gate in the Rust Host Agent

## Why

In standalone mode the Host Agent runs on an operator's machine with
mutations disabled by default: an operator who has not set
`OPUTE_STANDALONE_ALLOW_MUTATIONS=true` expects the agent to observe the host
and nothing more. The pinned Go baseline expresses the gate as an enumerated
list of tool names.

The owner decided (2026-10-02) that the Rust agent derives the gate from the
catalog's effect classification, the same source that already tells clients
what each tool does, and fails closed when a tool's effect is not known to be
`read`. This is a declared divergence (D10) under
the rule that the port may deviate from Go where the deviation is an
improvement. The Go agent is not changed.

## What Changes

- With standalone mode on and mutations disabled, a `tools/call` runs only
  when the tool's catalog effect is `read`.
  - For a tool the server publishes, that is the effect in its published
    descriptor (`_meta.capability.privilege`).
  - For a dispatchable tool the server does not publish, the `read` effect
    must be declared by the pinned contracts. An effect inferred because
    nothing was declared does not count.
  - Every tool in Go's standalone mutation list stays refused.
- Refused calls get the same error result and pipeline position as Go's
  gate today: after the catalog revision check and argument decoding, before
  lifecycle routing, task handling, admission and any side effect.
- Platform mode, standalone with mutations allowed, and the published
  catalog (names, descriptors, effects, revision) are unchanged.

## Capabilities

### New Capabilities

- `standalone-read-only-gate`: which calls a standalone agent with mutations
  disabled may run.

### Modified Capabilities

- `host-agent-contract`: "Typed target and effect admission" states the
  standalone gate by effect for the Rust agent.

## Non-goals

- No change to the Go Host Agent.
- No change to the catalog, to effect classification, or to how
  `OPUTE_STANDALONE_ALLOW_MUTATIONS` is read.
- No change to platform-mode authorization or admission.

## Impact

- **Rust:** `catalog::standalone_read_only` and the gate in `tools::call`.
- **Parity harness:** decision D10, a `standalone-read-only-gate` contract
  suite that sweeps the published catalog, and Rust canaries that restore a
  name-list gate.
- **Operators:** a standalone agent with mutations disabled refuses every
  non-read tool. Operators who need such a tool set
  `OPUTE_STANDALONE_ALLOW_MUTATIONS=true`, as for the tools Go already gates.
