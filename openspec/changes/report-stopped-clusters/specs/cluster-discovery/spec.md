## ADDED Requirements

### Requirement: A stopped cluster never reads as an empty inventory
The Host Agent SHALL NOT report a successful empty inventory while a cluster
it manages exists but cannot be probed.

#### Scenario: The only cluster's server is stopped
- **WHEN** a client calls `list_compute_clusters` while the only managed cluster's server is stopped
- **THEN** the result lists that cluster with its Incus status `Stopped`, its canonical URI and no node data; it is never a success with no clusters
