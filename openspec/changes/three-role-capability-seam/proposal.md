# Every capability follows the three-role seam

## Why

Go ADR 0002 requires every capability to have a Service Definition, a Service
Provider and a Consumer. The pinned Go agent (`wunderous/host-agents`
`ace7013df17528fee1bed13a1d70a132d6c5eb9b`) applies that rule only to
external provider capabilities. Its built-in host tools are a name-keyed
handler registry (`internal/tools/registry.go`) whose handlers all receive
the whole host service (legacy item I-12). The Rust agent copied that split:
`provider/mounts.rs` binds every mounted service to an MCP adapter, and
built-in tools are a name match in `tools.rs`.

M7 exposed the cost. Host-platform detection exists twice: the built-in
`detect_host_platform` and the `com.opute.hostos` provider wrap the same
`pkg/hostplatform.Detect()`, and the provider cannot be installed. The owner
decided on 2026-10-11 (decision D14) that every capability, built-in or
external, follows the three-role seam from now on, and that the capabilities
delivered in M1–M7 will be revised onto it.

## What Changes

This is a pure internal refactor of the Rust implementation in this
repository (`opute-host-agent`). No catalog entry, descriptor, effect, error,
catalog revision, admission order, default or durable record changes, and
the Go agent is not changed.

- Every Rust capability has a Service Definition in the one descriptor
  source, a Service Provider registered in a provider-neutral service
  context, and at least one Consumer.
- The service context is generalized so that a provider is either an
  in-process built-in implementation or a generation-bound MCP adapter.
  Built-in providers are mounted by the composition root; external ones are
  mounted by the M7 activation owner.
- Catalog projection and dispatch resolve registered services. No capability
  is reached by a tool-name match outside the service context.
- Built-in services do not take part in provider dependency injection or
  per-family publication and displacement, which keeps provider behaviour
  identical to Go.
- A seam verifier lists every capability's roles and fails on a missing role.
- The rule applies to every capability implemented or changed from
  2026-10-11 (M8, M9 and later). M1–M7 capabilities are listed as named seam
  gaps and revised later under their milestones' parity gates.

## Non-goals

- No change to the Go Host Agent or to any published catalog cell.
- No new or removed capability, endpoint, provider, mode, default or policy.
  A capability that cannot meet the seam without a contract change needs its
  own OpenSpec change. Examples are recipe-less provider activation and
  retiring the duplicated host-platform implementation.
- No change to the M7 exit gate or to how M7 treats `hostos` and `ollama`.
- No schedule for the M1–M7 revision; that is an owner decision.
- No change to durable state, packaging, or Platform integration.

## Capabilities

### New Capabilities

- `capability-seam`: the roles every Rust Host Agent capability must have, the
  shared registration path, the isolation of built-in services from provider
  families, and how compliance is verified and phased in.

### Modified Capabilities

None. `host-agent-contract` describes observable behaviour, and this change
alters none of it.

## Open questions for the owner

1. **Duplicate implementations.** Should host-platform keep both the built-in
   and the `hostos` implementation as two providers of one definition, with
   explicit selection, or should one be retired by a separate change?
2. **Recipe-less activation.** Go cannot install a provider that declares no
   recipe. Supporting it is a lifecycle contract change for its own proposal.
3. **Revision deadline.** Must the M1–M7 revision finish before the M12
   cutover rehearsal? The recommendation is yes, so cutover ships one
   architecture.

## Impact

- **Rust:** `provider/mounts.rs` becomes a provider-neutral service context.
  The composition root (`app.rs`) mounts built-in providers. `catalog.rs`
  and `tools.rs` resolve services instead of matching names, and domain code
  receives only the seams it injects.
- **Migration plan:** decision D14 in the milestones decision table, the seam
  in M8 and M9 exit gates, and a revision task for M1–M7.
- **Evidence:** a seam verifier receipt with `verify.py` and a canary. Each
  revision reruns the affected milestones' parity captures.
