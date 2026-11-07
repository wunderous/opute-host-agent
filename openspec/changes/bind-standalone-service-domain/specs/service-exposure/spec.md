## ADDED Requirements

### Requirement: A published service domain call can succeed
The Host Agent SHALL resolve the target of `configure_service_domain` and
`remove_service_domain` from the inputs their published schema declares,
in every serving mode that lists them.

#### Scenario: Standalone mode maps a hostname
- **WHEN** a standalone client calls `configure_service_domain` with every input its listed schema requires
- **THEN** the Ingress is created for the named service, or the call fails with a typed error about those inputs
