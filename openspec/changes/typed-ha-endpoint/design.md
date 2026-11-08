# Design: typed-ha-endpoint

Decided by the owner on 2026-11-08, from the options put to them.

## Enablement: off unless configured

The provider reads `OPUTE_K3S_CLUSTER_ENDPOINT` once. Only the value
`enabled` turns the capability on. Otherwise:
- the install manifest omits `ensure-cluster-endpoint` and
  `remove-cluster-endpoint`, and `ensure-ha-endpoint` has no `caBundleRef`
  input, so the published catalog is byte-for-byte today's;
- a direct call to either operation returns an error result with
  `structuredContent.code = "capability_unavailable"` before any effect.

## Resolution: the cluster's Incus bridge DNS

The endpoint resolves through the dnsmasq of the Incus network the servers'
`eth0` is attached to. The provider owns one block in that network's
`raw.dnsmasq`, between the lines `# opute-cluster-endpoint <cluster> begin`
and `# opute-cluster-endpoint <cluster> end`, with one
`host-record=<hostname>,<address>` per Ready etcd member. Lines outside the
block are kept as they are. Guests on the bridge resolve the hostname; a
host client queries the bridge's DNS address. No host resolver or root
change is made.

## TLS names

Each server gets the drop-in `/etc/rancher/k3s/config.yaml.d/50-opute-cluster-endpoint.yaml`
with `tls-san: ["<hostname>"]`. Opting out removes the drop-in, the
`k3s-serving` secret and each server's `server/tls/dynamic-cert.json`, so
the serving certificate is regenerated without the hostname. Either change
restarts `k3s` on one server at a time; each must be Ready again (at most
180 s) before the next. A server that does not return stops the rollout
with `rollout_incomplete` naming it; the remaining servers are untouched
and the DNS block lists only Ready members.

## The record and the CA reference

The record lives on the cluster, not in the provider (which keeps no state
on disk): the `kube-system` ConfigMap `opute-cluster-endpoint`, with the
hostname, the network, the cluster URI and the member addresses. Any
provider that can reach a server reads it with `k3s kubectl`, including a
server that has just joined. The CA reference `cluster-endpoint:<host>:<cluster>`
resolves to `server/tls/server-ca.crt` read from a Ready member; the CA is
never placed in a result, a record or a log. Opting out deletes the record.

`ensure-ha-endpoint` with `caBundleRef` resolves the hostname as a client on
the bridge does (`getent ahostsv4` in a Ready server) and probes each
address in turn with only the cluster CA trusted.

## Membership hooks

- `join-node`, once the new server is Ready, reads the record through it,
  writes its TLS name, restarts it and waits for Ready, then adds it to the
  DNS block (the sealed join material is unchanged).
- `remove-node` (including the quorum-recovery retirement) and
  `recover-quorum` itself rewrite the DNS block from the Ready etcd members,
  less the servers a reset left out of etcd, in the same operation.
- On a cluster that has not opted in, none of these touch TLS names, the
  DNS block or the record; where the capability is disabled they make no
  extra call at all.
