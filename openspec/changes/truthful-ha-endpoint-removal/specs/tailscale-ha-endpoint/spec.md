## ADDED Requirements

### Requirement: Removing an unknown HA endpoint names no target

When the Tailscale provider's `remove-ha-endpoint` is called with an
`endpointRef` it does not hold, it SHALL succeed with `deleted: true` and the
caller's `endpointRef`, and SHALL NOT report a `targetUri`. A result SHALL
name only a target the provider recorded for that reference.

#### Scenario: Unknown reference
- **WHEN** `remove-ha-endpoint` is called with a reference the provider does not hold
- **THEN** the result has `deleted: true` and `endpointRef` equal to the reference, and no `targetUri`

#### Scenario: Known reference
- **WHEN** `remove-ha-endpoint` is called with a reference the provider created
- **THEN** the result names the target that endpoint served and the reference
