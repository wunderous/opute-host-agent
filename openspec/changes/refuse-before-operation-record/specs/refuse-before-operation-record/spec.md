## ADDED Requirements

### Requirement: Task-aware calls refused at admission leave no durable record
The Rust Host Agent SHALL resolve the canonical resource binding and admit a
task-aware call before it persists any operation, task, plan or invocation
record for that call. A call refused at admission (a missing, malformed,
foreign-tenant, wrong-kind or unknown resource URI, or a host resource
coordinator refusal) SHALL preserve every database row present before the
request. It SHALL run no host command other than the read-only inventory
lookups that resolving the resource needs.

#### Scenario: Binding refused for a task-aware call
- **WHEN** a task-aware call names a missing, malformed, foreign-tenant or wrong-kind resource URI
- **THEN** the call returns a task whose terminal result is the typed `resource_binding` error, and durable rows and the command trace are unchanged

#### Scenario: Unknown target refused for a task-aware call
- **WHEN** a task-aware call names a well-formed resource URI that does not resolve
- **THEN** the task's terminal result is the typed `resource_binding` error, durable rows are unchanged, and only read-only inventory commands ran

#### Scenario: Restart after refused tasks
- **WHEN** the agent restarts after task-aware calls refused at admission
- **THEN** the durable state is identical to the state before those calls and the agent still accepts authenticated read-only calls

### Requirement: Refused tasks keep the Go wire contract
The Rust Host Agent SHALL answer a task-aware call refused at admission exactly
as the pinned Go agent does on the wire. It returns a task handle, and
`tasks/get` reports `completed` with the refusal as an `isError` tool result.

#### Scenario: Refusal observed through tasks/get
- **WHEN** a client polls a task whose call was refused at admission
- **THEN** the task is `completed` and its result carries `isError` and the typed refusal code
