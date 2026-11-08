# Report a stopped cluster in discovery (proposed)

Status: proposed. Owner question Q24
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Nothing here is implemented.

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

The owner chooses one:

- discovery lists every K3s instance it knows, with its instance status (for
  example `Stopped`) and no node data when it cannot probe; or
- a probe failure on a known instance makes discovery fail with an error,
  never an empty success.

Either way the page's two outcomes stay distinct for a stopped cluster.

## Impact

- `crates/host-agent/src/providers/k3s` (list-clusters).
- `e2e3.discovery` then asserts the chosen outcome for the stopped cluster.
