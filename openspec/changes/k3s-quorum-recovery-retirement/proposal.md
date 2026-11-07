# Retire dead K3s servers after quorum recovery (proposed)

Status: proposed. Owner question Q17
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented. The K3s provider's behaviour matches Go v0.2.4
(D26 port).

## Why

No spec covers the K3s provider's membership operations. Their contract is
the catalog (`opute.capability.kubernetes.recover-quorum` and `remove-node`)
and the provider code. e2e-3a journey 4 (`e2e3a.quorum-removal`) and its
exploration found the following on a three-server, embedded-etcd cluster
with two servers stopped:

- `recover-quorum` with `acknowledgeSingleMemberReset` resets etcd to one
  member on the surviving server and reports `quorumRestored: true`. It
  also reports `readyServers: 3`: the stopped servers' Node objects still
  carry their last Ready condition until the node grace period passes.
- `remove-node` proves quorum from Node conditions: the ready servers that
  remain must be a strict majority of the servers that remain. Once the
  stopped servers are NotReady, the membership is 3 servers with 1 ready.
  Removing either dead server would leave 1 ready of 2, so the provider
  refuses, although etcd already has a single member and the dead servers
  are no longer in it.
- In the exploration, removal succeeded only because it ran before the
  Ready conditions expired. The provider counted the dead servers as ready
  (`removedNodeWasReady: true`).

So after an acknowledged recovery, the typed path either retires dead
servers on stale observations or cannot retire them at all.

## What Changes

If the owner accepts Q17:

- after an acknowledged single-member reset, `remove-node` proves quorum
  from the etcd membership, not from Node conditions, so the operator can
  retire servers that are no longer members;
- `recover-quorum` reports the servers it observes as gone, rather than
  their stale Ready conditions.

## Impact

- `crates/host-agent/src/providers/k3s/membership.rs`, with Go parity
  recorded as an intended difference.
- No catalog change.
