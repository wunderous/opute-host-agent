## ADDED Requirements

### Requirement: An IPv4 wildcard bind listens on IPv4 only
When `HOST_MCP_BIND_HOST` is `0.0.0.0`, including Platform mode's default,
the Host Agent SHALL listen on one IPv4 socket bound to `0.0.0.0` and SHALL
NOT accept IPv6 connections on it. An empty host or `::` SHALL keep one
dual-stack IPv6 socket.

#### Scenario: Platform mode's default bind
- **WHEN** the agent starts with `HOST_MCP_BIND_HOST=0.0.0.0` and port 3014
- **THEN** its only listener is IPv4 `0.0.0.0:3014`, and a connection to `[::1]:3014` is refused
