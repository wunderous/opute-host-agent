## ADDED Requirements

### Requirement: restart_host_service restarts Opute user services only
The Host Agent SHALL restart only an `opute-` systemd user unit with
`restart_host_service`, without blocking, and report it `scheduled` with the
call's `uri`. It SHALL refuse any other unit before acting, with an error
that names `set_host_service_state` as the supported path. Its published
description and output schema SHALL match this.

#### Scenario: Restart Incus on the production-style install
- **WHEN** a client calls `restart_host_service` for `incus.service`
- **THEN** the call is refused before anything runs, and the error names `set_host_service_state`

#### Scenario: Restart an Opute user service
- **WHEN** a client calls `restart_host_service` for `opute-bridge.service`
- **THEN** the unit's restart is scheduled and the result, `status: scheduled` with the call's `uri`, passes the published output schema
