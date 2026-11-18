## ADDED Requirements

### Requirement: The public-MCP connector probes the agent's own resources in process
When `ensure_public_mcp_tunnel` or `ensure_public_mcp_quick_tunnel` probes the origin or the public endpoint, Host Agent SHALL authenticate with a token minted in its own store for the probed resource, valid for at most five minutes and revoked after the probe, and SHALL NOT call its token endpoint (owner decision D52, under D8).

#### Scenario: Probing the origin
- **WHEN** a connector tool probes its origin
- **THEN** the probe's bearer is a token in the agent's store bound to the origin's MCP resource, the token endpoint receives no request, the observation reports `authorizationMode` `in-process`, and the token is revoked afterwards

#### Scenario: Rollback from 0.4.0
- **WHEN** 0.3.0 runs after a rollback from 0.4.0 and reconciles a connector that 0.4.0 reconciled
- **THEN** the origin probe succeeds, as it does before the upgrade
