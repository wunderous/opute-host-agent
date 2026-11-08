# Retire dead K3s servers after quorum recovery

Status: decided (owner, 2026-11-07, D28). Owner question Q17
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md. The K3s provider's behaviour matches Go v0.2.4
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

Decided (D28): removal waits for recovery to settle.

- After an acknowledged single-member reset, `recover-quorum` records in the
  surviving server's state directory
  (`/var/lib/rancher/k3s/server/opute-quorum-recovery.json`) the servers the
  reset left out of etcd.
- `remove-node` for a recorded server waits, up to 3 minutes, until that
  server is observed NotReady; while it still reports Ready, recovery has
  not settled and the call refuses. Once settled, quorum is proved from the
  recovered membership (the servers the reset kept), not from all Node
  objects, and the server is retired. The record drops each retired server
  and is removed with the last one.
- Any other removal keeps the strict-majority proof. `recover-quorum`'s own
  report is unchanged.

## Impact

- `crates/host-agent/src/providers/k3s/membership.rs` (`quorum_after_recovery`,
  the recovery record, `remove_node`), an intended difference from Go.
- No catalog change.
