# Report a stopped cluster in discovery

Status: decided (owner, 2026-11-07, D28). Owner question Q24
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md.

## Why

The Use cases page says of cluster discovery: "An empty inventory and a
failed inventory probe are different outcomes; read the structured response
and error status." e2e-4a journey `e2e3.discovery` confirms the clear cases:

- no provider: an error ("no active Kubernetes provider is installed");
- a provider and no cluster: success, `total: 0`;
- an unreachable provider: an error naming the refused connection.

But with the cluster's only server stopped, `list_compute_clusters` succeeds
with `total: 0`, exactly like an empty inventory. The k3s provider lists the
Incus instances, runs `k3s --version` in each, and skips any instance where
that fails. Go v0.2.4's provider (`cmd/opute-provider-k3s` `listClusters`)
does the same; the Rust provider is its port. A caller cannot tell "no
clusters" from "my cluster is down".

## What Changes

Decided (D28): list it as stopped. The provider cannot probe a stopped
instance, and nothing on the instance marks it as K3s, so the Host Agent's
discovery, not the provider, keeps it listed: every cluster it registered
(from an earlier discovery) whose Incus instance still exists, and that the
provider did not return, is listed with its Incus status (for example
`Stopped`), its instance type and canonical URI, and no node data. A
registered cluster whose instance was deleted is not listed. `total`
counts these entries.

## Impact

- `crates/host-agent/src/clusterops.rs` (`Inventory::list_clusters`, the
  `known_clusters` seam) and `builtin.rs` (the seam from the registry and
  `incus list`). The k3s provider is unchanged.
- `e2e3.discovery` then asserts the chosen outcome for the stopped cluster.
