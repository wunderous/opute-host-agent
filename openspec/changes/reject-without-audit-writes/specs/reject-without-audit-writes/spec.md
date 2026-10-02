## ADDED Requirements

### Requirement: Rejected MCP calls leave no audit writes or execution effects
The Rust Host Agent SHALL reject missing or invalid credentials, unknown tools,
stale catalog revisions, undecodable or schema-invalid arguments, and calls
blocked by the standalone read-only gate before host commands or durable writes,
including `capability_invocations`. These cases SHALL preserve every database
row present before the request. Startup provisioning and accepted calls are
outside this requirement; D8 OAuth issuance auditing remains unchanged.

#### Scenario: Authentication rejected
- **WHEN** an MCP tools/call has no credential or an invalid credential
- **THEN** it returns the required authentication error without changing durable rows or issuing a host command

#### Scenario: Tool request rejected
- **WHEN** an authenticated call names an unknown tool, has a stale catalog revision, or has undecodable or schema-invalid arguments
- **THEN** it returns the required protocol or typed error without changing durable rows or issuing a host command

#### Scenario: Standalone mutation rejected
- **WHEN** a standalone call is blocked by the read-only gate
- **THEN** it returns the gate error without changing durable rows or issuing a host command

#### Scenario: Restart after rejection
- **WHEN** the agent restarts after the enumerated rejected requests
- **THEN** the durable state contains no rejection audit rows and the agent still accepts authenticated read-only calls

### Requirement: Rejections preserve subsequent read availability
The Rust Host Agent SHALL continue serving authenticated discovery, catalog
listing and `get_host_info` after the enumerated rejection paths on the same
running standalone instance.

#### Scenario: Read after rejection
- **WHEN** a running standalone agent has rejected the enumerated requests
- **THEN** authenticated discovery, tools/list and get_host_info still succeed with the same catalog revision and exact agent identity
