# inspect_workload

Status: **accepted (owner, 2026-11-11) as D34.** Goal m15.

## Why

The catalog publishes `inspect_workload` ("Inspect a Kubernetes Deployment
and return fields useful for workload forms"; output `workload`, an object
with no declared fields, and `uri`). Go v0.2.4 has no handler and Rust
answers `not_implemented`. The published schema does not say which fields
the workload carries, so this change states them.

## What Changes

A built-in Service Provider that reads one Deployment and returns a fixed
projection of it (below). Environment variables are returned by name and
reference only: a Deployment's env can carry secrets, and the agent never
returns a value it did not need to. The declared schemas are unchanged.

## Impact

`clusterops.rs` and the service context; a cluster-inspection journey on
the HA cluster.
