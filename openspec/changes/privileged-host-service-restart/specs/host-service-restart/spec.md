## ADDED Requirements

### Requirement: restart_host_service does what its description offers
The Host Agent SHALL either restart the named host-level service on the
production install or refuse with an error that names the supported path, and
its published description SHALL match what it does.

#### Scenario: Restart Incus on the production-style install
- **WHEN** a client calls `restart_host_service` for `incus.service` on an agent running as its unprivileged system-unit user
- **THEN** Incus is restarted and reported active, or the call is refused before acting with the supported alternative named
