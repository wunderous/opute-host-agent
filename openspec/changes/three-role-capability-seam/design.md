# Design: the three-role seam for every capability

## Context

See proposal.md for the motivation. The current structure shapes the
approach:

- **Go (`ace7013d`).** `internal/cordis` is a provider-neutral service
  context. Go registers only provider services
  (`internal/hostmcp/provider_service_plugin.go`) and the resource registry
  and resolver (`resource_services.go`) in it. Built-in tools are a
  name-keyed handler registry (`internal/tools/registry.go`) whose handlers
  receive the whole host service. Provider injection and family publication
  consider only provider services: `injectableServiceFamilies` filters on
  the provider service type.
- **Rust.** `provider/mounts.rs` holds generation-scoped services, but each
  `Service` carries an MCP `Adapter`, so a built-in implementation cannot be
  registered there. Built-in tools are a `match name` in `tools.rs`.
  Descriptors come from the one generated source (M3).
- **Constraints.** Parity with the pinned Go agent is the gate, and AGENTS.md
  rules out speculative trait layers and package-by-package translation.

## Goals / Non-Goals

**Goals:**

- One architecture for built-in and external capabilities: one service
  context, one Service Definition model, one invocation interface, one
  admission path and reverse-order disposal.
- Dispatch derived from mounted services.
- A seam verifier, and a canary that proves it can fail.
- Revision of every M1–M7 capability before M8 starts.

**Non-Goals:**

- Changing provider family semantics, dependency resolution, or the
  `atomic-provider-family-publication` decision.
- Resolving the duplicated host-platform capability or recipe-less provider
  activation. Each needs its own change.
- Porting provider executables (M9).

## Observable parity versus internal structure

| Observable, and gated by Go parity | Internal, and gated by the seam verifier |
| --- | --- |
| Published names, descriptors, effects, approval, idempotency, resource bindings and catalog revision for every M3 and M7 cell | Where descriptors are registered and resolved |
| Admission order and errors (M4 matrix), standalone gate (D10), D12 refusal | How dispatch reaches an implementation |
| Startup and shutdown outcomes (M1), provider lifecycle (M7) | Mount order of built-in providers |
| Durable rows and redaction (M5), plans (M6) | Which seams a domain implementation receives |

The `capability-seam` spec states both columns as requirements. The left
column is verified only by Go/Rust captures; the right column only by the
seam verifier.

## Decisions

The owner's rule (2026-10-11): there is one architecture and design pattern
for built-in and external capabilities. They may differ in distribution and
trust, not in how they are defined, registered, resolved or invoked.

1. **One service context.** `provider::mounts` holds every service. A
   mounted service's implementation is an `Implementation`: a provider
   generation's MCP adapter, or the built-in provider in process. Both are
   mounted by the same `register` step (manifest order, dependency
   resolution, key checks), resolved by the same lookups, and disposed in
   reverse order. *Alternative:* a separate built-in registry. Rejected: it
   would be the second pattern this change removes.
2. **One Service Definition model.** The built-in provider is described by
   an `InstallManifest` whose operation definitions come from the descriptor
   source, exactly as a provider's come from its validated manifest.
3. **Isolation by identity, not by special cases.** The built-in provider
   is `host-agent:incus`, the `implementation` Go already publishes on every
   built-in descriptor, and its families are `host-agent:incus.<service>`.
   Neither is a valid manifest identifier, so the shared family, injection
   and displacement rules can never connect a provider to a built-in
   service. The built-in generation is not in the provider lifecycle, so the
   active-generation rule also excludes it from provider injection. Go,
   whose built-ins are outside its service context, behaves the same way.
4. **One invocation interface.** Every in-process implementation receives
   an `Invocation`: the caller's `RunCtx` (cancellation, admitted
   reservation, task identity), the arguments, the resolved binding, the
   catalog the call was admitted against, and, for runs that take over the
   reservation, the claim flag. The MCP adapter receives the same context.
5. **Admission stays in front of resolution.** Revision check, standalone
   gate, decoding, binding and reservation run as today; the service
   context is consulted where a name match was. M4 error order is unchanged.
6. **Seams, not the whole host.** A built-in service's operations capture
   the seams the composition root builds them with. Lifecycle services hold
   a late-bound server handle, because the pipeline owners they drive exist
   only once the server is assembled. No invocation carries the server.

### Differences, all declared

| Difference | Kind | Why it stays |
| --- | --- | --- |
| The built-in provider is compiled in and mounted by the composition root; it has no install, recipe, durable generation row or provider status entry. | Distribution | Go publishes built-ins in its base catalog and records no generation for them; adding one would change `opute.provider.status`, durable state and the catalog revision. |
| The composition root disposes the built-in generation after accepted task workers drain; the provider runtime disposes provider generations when serving stops. | Distribution (ownership) | In-flight plan nodes keep reaching built-in tools until they finish, as in Go. |
| Built-in operations run in process, with no delegation token. | Trust | They are part of the agent. |
| Built-in results pass the declarative schema gate and write Go's built-in audit row; provider results pass result admission and write the provider audit row. | Go-observable contract | Both rows and errors are compared against Go. Unifying them is a contract change for its own proposal. |
| Lifecycle and task-input operations are routed before the task boundary by Go's routing table. | Go-observable contract | Go's `isLifecycleTool` and `requestTaskInputToolName` set the pipeline order clients observe. Routing selects the path; the service context selects the implementation. |

### The seam verifier

The verifier runs as a Rust test over the composition root (`app::compose`).
It walks the service context and cross-checks it against the descriptor
source and the published `tools/list` captured from Go. For every source
name without a mounted service, it calls each executor directly (the
dispatch executor and the lifecycle and task-input routes) and requires the
typed `not_implemented` error. Any other answer means an implementation was
reached outside the context: a gap. The committed gap list must equal the
computed one. A canary adds a name match in the dispatch executor and must
turn the verifier red.

## Invariant delta

| Class | Statement and owner | Evidence |
| --- | --- | --- |
| Preserve | Catalog, schemas, effects, errors, revision, admission order, generation affinity, provider family semantics, the single plan executor and durable state. Owner: Host Agent contracts. | Rerun Go/Rust parity captures of M1–M7 for each revised capability. |
| Introduce | Every capability has three verified roles and is reached only through the service context; M8 starts only when no gap remains. Owner: Host Agent core, decision D14. | The seam verifier receipt with `verify.py`, plus its canary. |
| Retire | Name-match dispatch for built-in capabilities, and handlers that receive the whole host service (I-12). Owner: Host Agent core. | Seam verifier reports no gap; parity unchanged. |

## Risks / Trade-offs

- [Dispatch indirection changes error ordering] → Admission stays ahead of
  resolution (decision 3). The M4 admission matrix is rerun for every
  revision.
- [Built-in mount order changes startup] → The M1 lifecycle captures
  (partial startup, reverse cleanup) are rerun after the composition change.
- [Built-ins leak into provider families] → Reserved `host-agent:`
  identities (decision 3), with fake-provider scenarios that depend on and
  publish into the built-in families and must be refused exactly as Go
  refuses them.
- [The revision delays M8] → Accepted by the owner. The revision runs one
  milestone at a time with its own captures, so progress stays attributable.
- [The verifier only proves registration, not correctness] → Correctness
  stays with the parity captures. The verifier is a structural check only.

## Migration Plan

1. Generalize the service context and mount built-ins with no capability
   moved. Rerun M1, M3, M4 and M7 captures.
2. Land the seam verifier and canary. Commit the gap report as the baseline.
3. Revise capabilities milestone by milestone (M3, M4, M6, M7). Rerun each
   milestone's captures and the secret sweep after each step.
4. When the verifier reports no gap, commit its receipt and open the M8 entry
   gate.

**Rollback and data preservation.** No step changes durable schemas, state
files, catalog revision or packaging. Each step is an isolated commit, so
rollback means reverting that commit and rerunning the same captures. No
production instance runs Rust before the cutover milestones, so no live
state is touched. A durable-state difference in any rerun capture blocks the
step instead of being migrated.

## Open Questions

None that change the specs, the approach or the tasks. Host-platform
duplication and recipe-less activation are out of scope and need their own
changes.
