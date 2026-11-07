## ADDED Requirements

### Requirement: A runtime warning does not fail a storage report
The Host Agent SHALL parse a container runtime's machine-readable report
from its standard output, and SHALL NOT fail `inspect_container_storage`,
`configure_oci_storage` or `cleanup_container_storage` because the runtime
wrote warnings to standard error.

#### Scenario: Podman without a user session
- **WHEN** a client calls `inspect_container_storage` and Podman warns on stderr that no systemd user session is available
- **THEN** the call returns the storage report, and the warning appears in no parsed field
