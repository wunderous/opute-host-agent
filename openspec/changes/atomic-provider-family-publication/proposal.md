# Preserve displaced providers when candidate publication fails

## Why

The pinned Go `registerProviderServicesForGeneration` displaces other providers' operations in the candidate's capability families before validating replacement schemas. If validation fails, the registry loses the displaced operations while the published catalog, executable map, and MCP tools remain unchanged. A later successful publication rebuilds the catalog from the registry and removes the previous provider's tools. The previous provider's generation can remain active throughout this sequence.

The isolated owning Go publication oracle reproduces this sequence with an invalid candidate schema and captures actual SDK `tools/list` alongside both catalog projections. Evidence is in `evidence/m7/provider-publication-diagnostic/family-displacement/`. This is a contract gap discovered during M7, not permission to change the migration baseline.

This proposal is **pending owner review**. Rust currently preserves the pinned ordering. No atomic-publication change, runtime promotion, or parity waiver is authorized by this proposal.

## What Changes

An approved implementation would validate the candidate and prepare family displacement before committing either change. Failure would retain the prior registry, published catalog, executable bindings, remembered manifests, and MCP projection. Successful publication would preserve the existing per-family exclusivity rule. Accepted calls would retain their generation bindings.

## Capabilities

### New Capabilities

- `atomic-provider-publication`: preservation of previous publication state after a failed provider replacement.

### Modified Capabilities

None.

## Impact

The owning catalog registry and Host Agent publication composition would require a coordinated commit boundary and fault tests. Activation rollback must account for providers displaced by the failed candidate, rather than restoring only the candidate's predecessor. The owner must decide how this contract is adopted in Go and Rust and how the deliberate baseline difference is recorded. Public provider activation remains unverified.
