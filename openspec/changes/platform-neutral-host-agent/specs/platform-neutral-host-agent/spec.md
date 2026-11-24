## ADDED Requirements

### Requirement: One serving profile

The Host Agent SHALL have exactly one serving profile, configured only by the
generic settings `HOST_MCP_BIND_HOST` (default `127.0.0.1`), `HOST_MCP_PORT`
(default `3014`), `OPUTE_HOST_ALLOW_MUTATIONS` (default `false`) and the
configured authentication (`MCP_AUTH_TOKEN` or the configured auth, required).
Its behaviour SHALL NOT depend on which client connects. At start it SHALL log
its effective bind, port and mutation policy once, from its own settings only.

#### Scenario: Started with only an id and a token
- **WHEN** the agent starts with `OPUTE_REMOTE_AGENT_ID` and a token, and nothing else
- **THEN** it serves on `127.0.0.1:3014` with read tools only, and nothing it publishes or prints names Platform

#### Scenario: A host exposed on purpose
- **WHEN** the operator sets bind `0.0.0.0`, port `3004` and mutations `true`
- **THEN** the agent serves there with mutations admitted, and behaves like any other host configured the same way

#### Scenario: Mutations not enabled
- **WHEN** a client calls a tool whose effect is not `read` and `OPUTE_HOST_ALLOW_MUTATIONS` is not `true`
- **THEN** the call returns the error result `mutations are disabled; set OPUTE_HOST_ALLOW_MUTATIONS=true` before any effect

#### Scenario: The effective configuration is visible
- **WHEN** the agent starts
- **THEN** it logs its effective bind, port and mutation policy once, from its own settings only

### Requirement: No mode setting exists

The Host Agent SHALL NOT read, validate, refuse, log or document
`OPUTE_AGENT_MODE`, `OPUTE_STANDALONE_ALLOW_MUTATIONS`, or any setting whose
purpose is to tell Platform-managed hosts apart from others. Such a variable,
like any variable the agent does not define, SHALL have no effect, and the
agent's code, help, schemas and errors SHALL contain no reference to it.

#### Scenario: A host still carries the old variable
- **WHEN** 0.4.0 starts with `OPUTE_AGENT_MODE=platform` in its environment and no other settings
- **THEN** it starts on the profile's defaults, exactly as if the variable were absent, and nothing it prints mentions the variable

### Requirement: Every orchestrator is an ordinary client

Any orchestrator, Platform included, SHALL reach the agent the way every MCP
client does: an endpoint and a credential the operator configures. The agent
SHALL hold no orchestrator-specific client id, tenant, enrollment flow,
routing or callback. OAuth machine clients, if used, SHALL come only from the
operator setting `OPUTE_OAUTH_CLIENTS` (ids and scopes), empty by default.

#### Scenario: Platform connects
- **WHEN** Platform is given a host's endpoint and token
- **THEN** it calls tools exactly as any client does, results and records are identical with or without it, and the agent id is reported unchanged

#### Scenario: No OAuth clients configured
- **WHEN** the agent starts without `OPUTE_OAUTH_CLIENTS`
- **THEN** no machine client is provisioned, and the configured bearer authentication works

### Requirement: Reserved names are operator configuration

Providers and tools SHALL contain no Platform hostname, domain, tunnel name or
port. A reserved-name guard SHALL read only `OPUTE_RESERVED_HOSTNAMES` (exact
names and `*.`-suffix patterns), empty by default. A reserved name SHALL be
refused with the typed refusal `reserved_hostname` before any effect.

#### Scenario: Nothing reserved
- **WHEN** a Cloudflare tunnel is requested for `mcp.opute.io` and nothing is reserved
- **THEN** it is accepted like any other hostname

#### Scenario: A deployment protects its names
- **WHEN** `OPUTE_RESERVED_HOSTNAMES=*.opute.io` is set and a tunnel is requested for `mcp.opute.io`
- **THEN** the tool refuses with `reserved_hostname` before calling the Cloudflare API

### Requirement: A test proves the boundary

A test in `make rust-check` SHALL fail if non-test Rust source, a published
tool schema or description, the default configuration, or the CLI help
contains a Platform-specific identifier. Forbidden identifiers include the
mode variables, `platform` as a behaviour switch, `opute.io`, `opute-mcp-host`
and tenant fields. The product name and the `OPUTE_` prefix are allowed. The
only test-side mention allowed is a test proving the old variable has no
effect.

#### Scenario: A Platform reference creeps back
- **WHEN** a change adds a Platform default or name to a provider
- **THEN** `make rust-check` fails, naming the file and the identifier
