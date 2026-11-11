## ADDED Requirements

### Requirement: list_secret_keys returns a Secret's key names, never values
The Rust Host Agent SHALL answer `list_secret_keys` for a Secret in a cluster the call's `uri` resolves to: it SHALL return the names of the Secret's `data` and `stringData` keys, sorted and without duplicates, and the resolved `uri`, and SHALL NOT return, log or store any value (owner decision D32). `namespace` defaults to `default`. A cluster that does not resolve, or a Secret that does not exist, SHALL end the call with a typed tool error.

#### Scenario: Keys of a Secret
- **WHEN** a client calls `list_secret_keys` for a Secret with keys `password` and `user`
- **THEN** the result is `{"keys": ["password", "user"], "uri": <the cluster uri>}`, and no value appears in the result, the audit record or the agent's log

#### Scenario: Unknown Secret
- **WHEN** the Secret does not exist in the namespace
- **THEN** the call ends with a typed tool error naming the Secret, and nothing is written to the cluster
