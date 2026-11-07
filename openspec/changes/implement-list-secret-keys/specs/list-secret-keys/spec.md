## ADDED Requirements

### Requirement: list_secret_keys returns key names only
The Host Agent SHALL answer `list_secret_keys` on a registered cluster target
with the named Secret's data keys and SHALL NOT return, log or persist any
Secret value.

#### Scenario: A Secret with one key
- **WHEN** a client calls `list_secret_keys` for a Secret whose data holds `apiKey`
- **THEN** the result's `keys` is `["apiKey"]`, and the value appears in no result, record or log

#### Scenario: Unknown Secret
- **WHEN** the named Secret does not exist on the target
- **THEN** the call fails closed with a typed error and no invented result
