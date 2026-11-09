## ADDED Requirements

### Requirement: The HA registration endpoint is optional and off by default
The K3s provider SHALL offer the cluster endpoint as an opt-in capability.
No cluster SHALL get an endpoint unless a client explicitly calls
`ensure-cluster-endpoint` for that cluster with an explicit hostname; no
default hostname exists, and no other operation (provision, join,
recover-quorum, remove) creates one. A cluster without an endpoint SHALL
behave exactly as today: clients address a server directly, and
`ensure-ha-endpoint` verifies only an endpoint the caller supplies. Without
the opt-in, the provider SHALL require no new configuration, package, daemon
or privilege, and SHALL change no TLS names or name resolution. The
capability SHALL be published only where the deployment enables it
(`OPUTE_K3S_CLUSTER_ENDPOINT=enabled`).

#### Scenario: A cluster built without opting in
- **WHEN** a three-server cluster is built and no client calls `ensure-cluster-endpoint`
- **THEN** no server certificate, resolver entry or record names a registration hostname, no server is restarted for TLS names, and every existing journey (build, workload, lose-one, quorum-removal) passes unchanged

#### Scenario: The capability is not configured on the host
- **WHEN** the provider runs without `OPUTE_K3S_CLUSTER_ENDPOINT=enabled`
- **THEN** `ensure-cluster-endpoint` and `remove-cluster-endpoint` are not published in `tools/list`, the catalog is unchanged, and a direct call is refused with the typed `capability_unavailable` error before any effect

### Requirement: An opted-in endpoint is stable across server loss
When a client opts a cluster in, the operation SHALL make the explicit
hostname appear in the TLS subject alternative names of every server and
resolve, on the cluster's Incus bridge DNS, to every Ready etcd member and
to no other address. It SHALL return the endpoint URL and the cluster CA by
reference (a resource URI), never inline in a result, a record or a log. A
repeat with the same hostname and unchanged membership SHALL report
unchanged and restart nothing.

#### Scenario: Opting a three-server cluster in
- **WHEN** a client calls `ensure-cluster-endpoint` with hostname `k3s.e2e.test`
- **THEN** each server's certificate lists it, it resolves to the three servers, and `ensure-ha-endpoint` with the returned CA reference verifies it without `UnknownIssuer`

#### Scenario: One server is lost
- **WHEN** one of three servers stops
- **THEN** a read and a typed API write through the endpoint succeed within the client's retry window

### Requirement: An opted-in endpoint follows membership
For a cluster that has opted in, join, `remove-node` and quorum-recovery
retirement SHALL update the endpoint's resolution and the TLS names in the
same operation. The endpoint SHALL never resolve to a retired server. For a
cluster that has not opted in, these operations SHALL not touch TLS names
or resolution.

#### Scenario: A server is retired from an opted-in cluster
- **WHEN** a dead server is retired after quorum recovery settles
- **THEN** the endpoint no longer resolves to it, and `ensure-ha-endpoint` still verifies

### Requirement: Opting out is a typed operation
`remove-cluster-endpoint` SHALL remove the hostname from every server's TLS
names and from resolution, and SHALL delete the CA reference record.
Afterwards the cluster SHALL be indistinguishable from one that never opted
in. A repeat SHALL report unchanged.

#### Scenario: Opting out
- **WHEN** a client removes the endpoint from an opted-in cluster
- **THEN** no certificate, resolver entry or record names the hostname, and clients that address a server directly keep working

### Requirement: Changing TLS names is an explicit, bounded effect
Opting in or out restarts each K3s server. The operation SHALL declare that
effect, restart one server at a time, and wait for each to be Ready again
before the next. If a server does not return to Ready, it SHALL stop with
the typed `rollout_incomplete` error naming that server, leave the
remaining servers untouched, and keep the endpoint resolving only to Ready
servers.

#### Scenario: A server does not return after its restart
- **WHEN** a server is not Ready within the rollout's wait after its restart
- **THEN** the operation fails with `rollout_incomplete` naming it, no later server is restarted, and the DNS block lists only Ready servers
