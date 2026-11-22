## ADDED Requirements

### Requirement: Provider executables listen on loopback only
A provider executable SHALL listen for MCP requests on `127.0.0.1` at its configured port only, and SHALL NOT listen on any other interface (owner decision D53).

#### Scenario: A guest on a bridge
- **WHEN** a provider runs on a host and a guest on one of the host's bridges connects to the host's bridge address at the provider's port
- **THEN** the connection is refused, and the provider runs nothing for it

#### Scenario: The agent calls its provider
- **WHEN** the Host Agent calls an installed provider at `http://127.0.0.1:<port>/mcp`
- **THEN** the provider answers as before
