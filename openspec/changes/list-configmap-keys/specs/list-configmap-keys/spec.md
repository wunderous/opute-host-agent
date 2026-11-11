## ADDED Requirements

### Requirement: list_configmap_keys returns a ConfigMap's key names, never values
The Rust Host Agent SHALL answer `list_configmap_keys` for a ConfigMap in the cluster the call's `uri` resolves to: it SHALL return the names of the ConfigMap's `data` and `binaryData` keys, sorted and without duplicates, and the resolved `uri`, and SHALL NOT return any value (owner decision D33). `namespace` defaults to `default`. A cluster that does not resolve, or a ConfigMap that does not exist, SHALL end the call with a typed tool error.

#### Scenario: Keys of a ConfigMap
- **WHEN** a client calls `list_configmap_keys` for a ConfigMap with keys `b.conf` and `a.conf`
- **THEN** the result is `{"keys": ["a.conf", "b.conf"], "uri": <the cluster uri>}`

#### Scenario: Unknown ConfigMap
- **WHEN** the ConfigMap does not exist in the namespace
- **THEN** the call ends with a typed tool error naming the ConfigMap
