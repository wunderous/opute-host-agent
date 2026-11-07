## ADDED Requirements

### Requirement: An HA K3s cluster has a typed stable endpoint
The Host Agent SHALL offer a typed way to establish one endpoint for an
embedded-etcd K3s cluster that keeps serving the Kubernetes API while any one
server is down, and that `ensure-ha-endpoint` can verify with the host's trust
material.

#### Scenario: One server stopped
- **WHEN** a three-server cluster has a stable endpoint and one server is stopped
- **THEN** a typed read and write through that endpoint succeed, and `ensure-ha-endpoint` reports it available
