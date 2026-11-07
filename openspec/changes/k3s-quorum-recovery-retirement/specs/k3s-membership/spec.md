## ADDED Requirements

### Requirement: Dead servers can be retired after an acknowledged quorum recovery
After `recover-quorum` has reset embedded etcd to a single member, the K3s
provider SHALL let `remove-node` retire a server that is no longer an etcd
member, and SHALL prove quorum from the etcd membership rather than from stale
Node conditions.

#### Scenario: Two servers lost, quorum recovered
- **WHEN** two of three servers are stopped, `recover-quorum` has reset etcd on the third, and the stopped servers are NotReady
- **THEN** `remove-node` retires each stopped server, and the cluster keeps serving from the remaining one

#### Scenario: A live majority is protected
- **WHEN** no reset happened and removing a server would leave the ready servers without a strict majority
- **THEN** `remove-node` refuses before deleting anything
