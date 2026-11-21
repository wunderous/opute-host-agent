## ADDED Requirements

### Requirement: inspect_workload returns a fixed projection of one Deployment
The Rust Host Agent SHALL answer `inspect_workload` for a Deployment in the cluster the call's `uri` resolves to (owner decision D34). `namespace` defaults to `default`. The result's `workload` SHALL hold exactly: `name`, `namespace`, `replicas` (desired), `readyReplicas`, `availableReplicas`, `labels`, `selector` (match labels), and `containers`, one entry per container in the pod template with `name`, `image`, `ports` (each `containerPort`, `protocol` and `name` when set), `env` (each variable's `name`, and for a reference its source kind and the referenced object and key, never a literal `value`), `envFrom` (the referenced ConfigMaps and Secrets by name), and `resources` (requests and limits). The result SHALL NOT contain any literal environment value. A cluster that does not resolve, or a Deployment that does not exist, SHALL end the call with a typed tool error.

#### Scenario: A Deployment with a literal and a Secret-backed variable
- **WHEN** a client inspects a Deployment whose container sets `MODE=prod` and `TOKEN` from a Secret key
- **THEN** `workload.containers[0].env` lists `MODE` with no value and `TOKEN` with its Secret name and key, and no value appears anywhere in the result

#### Scenario: Unknown Deployment
- **WHEN** the Deployment does not exist in the namespace
- **THEN** the call ends with a typed tool error naming the Deployment
