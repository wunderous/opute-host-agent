## ADDED Requirements

### Requirement: Dead servers are retired once quorum recovery settles
After `recover-quorum` has reset embedded etcd to a single member, the K3s
provider SHALL let `remove-node` retire a server the reset left out of etcd
once that server is observed NotReady, proving quorum from the servers the
reset kept. While such a server still reports Ready, `remove-node` SHALL wait
a bounded time and then refuse before deleting anything.

#### Scenario: Two servers lost, quorum recovered
- **WHEN** two of three servers are stopped, `recover-quorum` has reset etcd on the third, and `remove-node` is called for each stopped server
- **THEN** each call waits until the server is NotReady, retires it with `removedNodeWasReady: false`, and the cluster keeps serving from the remaining one

#### Scenario: Recovery has not settled
- **WHEN** a server the reset left out still reports Ready after the bound
- **THEN** `remove-node` refuses before deleting anything, saying recovery has not settled

#### Scenario: A live majority is protected
- **WHEN** no reset happened and removing a server would leave the ready servers without a strict majority
- **THEN** `remove-node` refuses before deleting anything
