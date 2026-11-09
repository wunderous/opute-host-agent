# An optional, typed stable endpoint for an HA K3s cluster

Status: decided by the owner on 2026-11-08 (answer to owner question Q19,
[rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)),
with the design choices in [design.md](design.md).

## Why

e2e-3a journey 1 (`e2e3a.build`, run `e2e-3a-r5`) showed that
`opute.capability.kubernetes.ensure-ha-endpoint` only verifies an endpoint
the caller already has. Against one server's own K3s API it fails closed
with `UnknownIssuer`, and nothing provides an address that survives the
loss of a server. A client that loses the server it addresses has to pick
another one itself.

## What Changes

- The K3s provider offers a cluster endpoint as an opt-in capability, off by
  default and published only where the deployment enables it
  (`OPUTE_K3S_CLUSTER_ENDPOINT=enabled`). Without that, the provider's
  catalog and behaviour are exactly today's.
- `opute.capability.kubernetes.ensure-cluster-endpoint` opts one cluster in
  with an explicit hostname: the hostname goes into every server's TLS
  names, resolves on the cluster's Incus bridge DNS to every Ready etcd
  member, and the result returns the endpoint URL and a CA reference
  (`cluster-endpoint:<host>:<cluster>`), never the CA itself.
- `opute.capability.kubernetes.remove-cluster-endpoint` opts out and leaves
  the cluster indistinguishable from one that never opted in.
- For an opted-in cluster, join, `remove-node` and quorum-recovery
  retirement keep TLS names and resolution in step with membership.
- `ensure-ha-endpoint` accepts an optional `caBundleRef` (only where the
  capability is enabled) and trusts that CA for its probe.
- Changing TLS names restarts servers one at a time, waiting for each to be
  Ready, and stops with the typed `rollout_incomplete` error naming a server
  that does not return.

Out of scope: an external load balancer, a VIP or public DNS (Platform or
the operator own those); clusters without embedded etcd; any default
hostname or implicit opt-in.

## Impact

- `crates/host-agent/src/providers/k3s/` (a new `endpoint.rs`, membership
  hooks, the manifest).
- The default catalog is unchanged; the enabled catalog adds two operations
  and one optional input.
- Journeys: the existing e2e-3a journeys run unchanged against the rebuilt
  provider (the capability is off), and a new journey `e2e3a.endpoint` opts
  in, loses a server, retires it and opts out.
