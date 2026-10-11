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

- One provider-neutral service context holding both built-in and external
  providers, with one admission path and reverse-order disposal.
- Catalog projection and dispatch derived from registered services.
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

1. **Generalize the existing service context; don't add a second
   registry.** `Service` will hold an implementation that is either a
   built-in handler or an MCP adapter. The generation-keyed mount, dispose
   and dependency code stays one owner.
   *Alternative:* a separate built-in registry beside `Mounts`. Rejected:
   it would recreate the two paths this change removes.
2. **Built-in services are mounted by the composition root as a fixed
   generation.** They live in the same context, under a namespace that
   provider injection and family publication filter out, exactly as Go's
   type filter excludes resource services. That keeps the "Built-in services
   stay outside provider families" requirement true by construction.
   *Alternative:* expose built-ins as injectable families. Rejected: it
   changes provider dependency outcomes and displacement, which is
   observable and belongs to a separate change.
3. **Admission stays in front of resolution.** Revision check, standalone
   gate, argument decoding, binding resolution and reservation run as today.
   The service is resolved where the name match is now. This keeps M4 error
   ordering unchanged.
4. **A domain implementation receives only the seams it declares.** This
   closes I-12. Seams are concrete structs unless two real implementations
   or a contract test double exist. That follows AGENTS.md's rule against
   speculative traits.
5. **The verifier reads the running agent's registry, not source text.** A
   debug-only inventory endpoint is not acceptable because it would change
   the wire. The verifier runs as a Rust test that builds the composition
   root and walks the service context. It cross-checks the result against
   the descriptor source and the published `tools/list` from the captures.
   The canary adds a name-match branch and must turn the verifier red.

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
- [Built-ins leak into provider families] → The namespace filter (decision
  2), with a fake-provider scenario that depends on a family a built-in also
  implements and must match Go.
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
