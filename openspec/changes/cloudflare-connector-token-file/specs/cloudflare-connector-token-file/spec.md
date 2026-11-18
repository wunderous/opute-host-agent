## ADDED Requirements

### Requirement: A Cloudflare connector reads its run token from a private file
Every cloudflared connector the Rust Cloudflare provider installs SHALL read its run token with `--token-file` from a file of mode 0600, and neither its unit nor its arguments SHALL hold the token (owner decision D48).

#### Scenario: A host-placed and an HA connector
- **WHEN** the provider installs a host-placed connector or an HA connector in a guest
- **THEN** the unit's `ExecStart` names `--token-file` and a 0600 file holding the run token, and the unit holds no token
