## ADDED Requirements

### Requirement: A tool's published input schema declares every input it reads
The Host Agent SHALL declare in a tool's published `inputSchema` every input
the tool reads, and SHALL NOT require an input the schema does not declare.

#### Scenario: Restart a host service with its declared inputs
- **WHEN** a client calls `restart_host_service` with exactly the inputs its schema requires
- **THEN** the call is not refused for a missing undeclared input
