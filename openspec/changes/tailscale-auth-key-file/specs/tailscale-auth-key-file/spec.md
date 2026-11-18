## ADDED Requirements

### Requirement: The Tailscale join keeps its auth key off argv
The Rust Tailscale provider's join script SHALL pass the auth key to `tailscale up` through `--auth-key=file:/dev/stdin` and SHALL NOT put it in any process's arguments (owner decision D49).

#### Scenario: A guest joins
- **WHEN** the join script runs with the auth key on its stdin
- **THEN** `tailscale up` reads the key from its stdin and no process's arguments hold it
