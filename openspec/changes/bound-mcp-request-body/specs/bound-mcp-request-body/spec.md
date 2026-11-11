## ADDED Requirements

### Requirement: An MCP request body is read only up to its limit
After a `/mcp` request passes the method, Origin and bearer gates, the Rust Host Agent SHALL read at most 4 MiB (4194304 bytes) of its body. A larger body SHALL be refused with HTTP 413 and the text `request body exceeds 4194304 bytes` before the JSON-RPC envelope is parsed, without reading or holding the rest of it (owner decision D29). A body of at most 4 MiB SHALL be handled as before.

#### Scenario: Oversized body
- **WHEN** an authenticated client declares a 64 MiB body and has sent 5 MiB of it
- **THEN** the agent answers 413 with the limit text without waiting for the rest, and its memory does not grow with the declared size

#### Scenario: Body at the limit
- **WHEN** an authenticated client sends a body of exactly 4 MiB
- **THEN** the agent reads it and applies the usual envelope and SDK checks
