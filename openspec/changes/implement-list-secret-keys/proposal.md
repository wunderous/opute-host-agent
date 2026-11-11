# Implement list_secret_keys (proposed)

Status: **accepted (owner, 2026-11-11) as D32**, answering owner question Q18
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented on the `rust-0.4.0` branch (goal m15).

## Why

e2e-3a journey 2 (`e2e3a.workload`, run `e2e-3a-r5`) had to show a
Kubernetes Secret's key, never its value, through `list_secret_keys`. That
pass condition cannot be met:

- the catalog publishes `list_secret_keys` (input `uri`, `secretName`,
  `namespace`; output `keys`, `uri`) in standalone and platform mode;
- Go v0.2.4 has no handler for it. It is not in `schemas/standalone-tools.json`
  and no Go source names it;
- Rust keeps it published and fails every call closed with `not_implemented`,
  as the capability-seam spec requires. The seam verifier registers it as
  `unported` (`evidence/seam/final/report.json`).

The journey therefore reads the key with `exec_cluster_command` and a
template that prints key names only. That works, but it is a raw kubectl
call, not the typed, schema-declared read the catalog promises.

## What Changes

A built-in Service Provider for `list_secret_keys`
on a registered cluster target:

- it returns the Secret's data keys, sorted, and never a value;
- an unknown cluster or Secret gets a typed error;
- the declared output schema is unchanged.

This adds a capability Go never had, so it is a product decision, not part
of the language migration (AGENTS.md).

## Impact

- `crates/host-agent/src/clusterops.rs` and the service context; the seam
  verifier then reports the tool as compliant.
- Rerun e2e3a.workload with `list_secret_keys` as its pass condition.
