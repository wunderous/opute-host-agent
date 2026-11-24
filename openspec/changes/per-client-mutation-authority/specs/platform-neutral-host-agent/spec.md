## MODIFIED Requirements

### Requirement: One serving profile

The Host Agent SHALL have exactly one serving profile, configured only by the
generic settings `HOST_MCP_BIND_HOST` (default `127.0.0.1`), `HOST_MCP_PORT`
(default `3014`), the configured authentication (`MCP_AUTH_TOKEN` or the
configured auth, required) and the `--allow-mutations` flag (default off),
which lets only the bootstrap bearer mutate (`per-client-mutation-authority`,
decision D55). Its behaviour SHALL NOT depend on which client connects beyond
each caller's own grant. At start it SHALL log its effective bind, port and
whether the bootstrap bearer may mutate once, from its own settings only.

#### Scenario: Started with only an id and a token
- **WHEN** the agent starts with `OPUTE_REMOTE_AGENT_ID` and a token, and nothing else
- **THEN** it serves on `127.0.0.1:3014` with read tools only, and nothing it publishes or prints names Platform

#### Scenario: A host exposed on purpose
- **WHEN** the operator sets bind `0.0.0.0` and port `3004` and starts the agent with `--allow-mutations`
- **THEN** the agent serves there, the bootstrap bearer's mutations are admitted, and it behaves like any other host configured the same way

#### Scenario: Mutations not enabled
- **WHEN** the bootstrap bearer calls a tool whose effect is not `read` on an agent started without `--allow-mutations`
- **THEN** the call returns an error result that starts `mutation refused: `, names the tool, its effect and `--allow-mutations`, before any effect

#### Scenario: The effective configuration is visible
- **WHEN** the agent starts
- **THEN** it logs its effective bind, port and whether the bootstrap bearer may mutate once, from its own settings only
