## ADDED Requirements

### Requirement: One ensure call settles the user service supervisor
The Host Agent SHALL report `ensure_host_service_supervisor` as ready only
when the user service manager answers, and SHALL wait a bounded time for a
user manager it has just made persistent before it reports.

#### Scenario: First call on a host with no user manager
- **WHEN** a client calls `ensure_host_service_supervisor` and lingering was not enabled
- **THEN** the call enables lingering, waits for the user bus within its bound, and reports ready, or fails with a timeout error naming the bus
