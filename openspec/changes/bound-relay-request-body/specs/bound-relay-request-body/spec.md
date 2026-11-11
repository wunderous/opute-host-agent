## ADDED Requirements

### Requirement: The serving relay reads a request body only up to 32 MiB
After a relay request passes its source, path and bearer gates, the Rust Host Agent SHALL read at most 32 MiB (33554432 bytes) of its body. A larger body SHALL be refused with HTTP 413 and the text `request body exceeds 33554432 bytes` without reading or holding the rest of it (owner decision D41). A body of at most 32 MiB SHALL be forwarded as before, and the response SHALL not be bounded.

#### Scenario: Oversized inference body
- **WHEN** a caller with the relay's bearer declares a 1 GiB body and has sent 33 MiB of it
- **THEN** the relay answers 413 with the limit text without waiting for the rest, and the agent's memory does not grow with the declared size

#### Scenario: Body at the limit
- **WHEN** a caller sends a body of exactly 32 MiB
- **THEN** the relay forwards it upstream as before

### Requirement: A provider's MCP request body is read only up to its limit
A Rust provider executable's `/mcp` SHALL read at most 4 MiB (4194304 bytes) of a request body and SHALL refuse a larger one with HTTP 413 and the text `request body exceeds 4194304 bytes` before holding it (D41, applying D29). A body of at most 4 MiB SHALL be handled as before.

#### Scenario: Oversized provider body
- **WHEN** a client declares a 64 MiB body to a provider's `/mcp` and has sent 5 MiB of it
- **THEN** the provider answers 413 with the limit text without waiting for the rest
