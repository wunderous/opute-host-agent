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
on disk): `/var/lib/rancher/k3s/server/opute-cluster-endpoint.json` on each
server, with the hostname, the network and the member addresses. Any
provider that can reach a server can read it, so a join on another host
sees it. The CA reference `cluster-endpoint:<host>:<cluster>` resolves to
`server/tls/server-ca.crt` read from a Ready server named in the record; the
CA is never placed in a result, a record or a log. Opting out deletes the
record.

## Membership hooks

- `prepare-join` on an opted-in cluster carries the hostname in the
  invitation; `join-node` adds the TLS name before K3s starts, then adds
  the new server to the DNS block once it is Ready.
- `remove-node` and the quorum-recovery retirement remove the server from
  the DNS block in the same operation, before reporting.
- On a cluster that has not opted in, none of these touch TLS names, the
  DNS block or the record.
