# list_configmap_keys

Status: **accepted (owner, 2026-11-11) as D33.** Goal m15.

## Why

The catalog publishes `list_configmap_keys` (input `uri`, `configMapName`,
`namespace`; output `keys`, `uri`), but Go v0.2.4 has no handler and Rust
answers `not_implemented`. A client sees the tool and every call is refused.

## What Changes

A built-in Service Provider, the ConfigMap twin of `list_secret_keys` (D32):
the names of a ConfigMap's `data` and `binaryData` keys, sorted, never a
value. The declared schemas are unchanged.

## Impact

`clusterops.rs` and the service context; a cluster-inspection journey on
the HA cluster.
