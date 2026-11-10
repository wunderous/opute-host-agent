## ADDED Requirements

### Requirement: A full state directory refuses writes and recovers
When the agent's state directory has no free space, the Rust Host Agent SHALL refuse a call that must write durable state with a typed error, SHALL leave no half-written record, SHALL keep serving reads, and SHALL accept writes again once space is freed, without a restart (owner decision D31).

#### Scenario: Disk full during a write
- **WHEN** a client calls a durable tool while the state directory is full, and space is freed afterwards
- **THEN** the call is refused with a typed error, reads still answer, the store's integrity check passes, and the same call then succeeds

### Requirement: A damaged state store stops startup
When `state.db` cannot be opened as a valid store, the Rust Host Agent SHALL refuse to start: it exits non-zero, binds no listener and writes nothing (owner decision D31).

#### Scenario: Truncated store
- **WHEN** the agent starts on a truncated `state.db`
- **THEN** it exits non-zero without serving, and after the store is restored it starts with its records intact

### Requirement: A failing provider command ends the call, not the agent
When a host or provider command the agent runs hangs or prints output it cannot decode, the Rust Host Agent SHALL end the call with a typed tool error within that command's timeout and SHALL keep serving other calls (owner decision D31).

#### Scenario: Hanging command
- **WHEN** the command behind a tool call never exits
- **THEN** the call returns a typed error within its timeout, and the next call is served

#### Scenario: Garbage output
- **WHEN** the command prints output that is not the expected format
- **THEN** the call returns a typed error, and the next call is served

### Requirement: A clock stepped back does not break the agent
When the host clock steps back, the Rust Host Agent SHALL keep serving, and its reservations and issued tokens SHALL stay valid (owner decision D31).

#### Scenario: Clock steps back an hour
- **WHEN** the host clock is set one hour back while the agent runs
- **THEN** authenticated calls still succeed and admission works as before
