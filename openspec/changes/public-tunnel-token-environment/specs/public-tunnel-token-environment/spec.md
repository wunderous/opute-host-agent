## ADDED Requirements

### Requirement: The public-MCP connector reads its token from its environment
The unit `ensure_public_mcp_tunnel` installs SHALL pass the tunnel token to cloudflared in its `TUNNEL_TOKEN` environment variable, read from the 0600 token file, and SHALL NOT put it in cloudflared's arguments (owner decision D47).

#### Scenario: A connector starts
- **WHEN** the connector unit's command runs with the token file as its environment
- **THEN** cloudflared's arguments are `tunnel --no-autoupdate run` and its `TUNNEL_TOKEN` is the token
