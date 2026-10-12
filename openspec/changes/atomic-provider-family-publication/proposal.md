# Preserve displaced providers when candidate publication fails

## Why

The pinned Go `registerProviderServicesForGeneration` displaces other providers' operations in the candidate's capability families before validating replacement schemas. If validation fails, the registry loses the displaced operations while the published catalog, executable map, and MCP tools remain unchanged. A later successful publication rebuilds the catalog from the registry and removes the previous provider's tools. The previous provider's generation can remain active throughout this sequence.

The isolated owning Go publication oracle reproduces this sequence with an invalid candidate schema and captures actual SDK `tools/list` alongside both catalog projections. Evidence is in `evidence/m7/provider-publication-diagnostic/family-displacement/`. This is a contract gap discovered during M7, not permission to change the migration baseline.

**Decided (owner, 2026-10-12): make it atomic in Rust, recorded as D16.** Go is not changed. The difference is declared in the public suite and waives no other parity gate.

## What Changes

Rust validates the candidate and prepares family displacement before committing either change:

- The registry commits a candidate's family displacement together with its replacement, under one lock and after every validation (`Registry::replace_generation_displacing` and `replace_provider_displacing`). A rejected candidate leaves the prior registry, published catalog, executable bindings, remembered manifests and MCP projection unchanged.
- A candidate mounts first, with dependencies resolved as they will be after the displacement, and other providers' services in its families are displaced only once it has committed (`Mounts::mount_displacing`). A failed mount leaves every other family owner mounted.
- Activation rollback restores every family owner the activation displaced from its remembered manifest and active generation, not only the candidate's own predecessor (`Activation::restore_owners`).

Successful publication keeps the existing per-family exclusivity rule. Calls accepted before a replacement keep their generation bindings and complete through the displaced provider.

## Capabilities

### New Capabilities

- `atomic-provider-publication`: preservation of previous publication state after a failed provider replacement.

### Modified Capabilities

None.

## Impact

The owning catalog registry, mounts and activation share one commit boundary. Three public scenarios cover it: `provider-lifecycle.family.rejected-candidate-keeps-owner`, `family.rollback-restores-owner` and `family.replacement-keeps-accepted-call`. The first two diverge from Go under the declared D16 rules in `tools/parity/divergences.json`, with per-side expectations; the third is compared exactly. The M5 shape check applies D16's database companions only to the two divergent scenarios.
