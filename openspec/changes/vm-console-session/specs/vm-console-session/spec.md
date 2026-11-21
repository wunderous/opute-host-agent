## ADDED Requirements

### Requirement: A console session runs the instance's console
The Rust Host Agent SHALL answer `stream_vm_console` for the instance the call's `uri` resolves to by starting `incus console <instance>` on a pseudo-terminal sized `width` by `height` when both are given, after crossing the admission boundary with the descriptor's declared cost, which it SHALL release when the call returns (owner decision D38). It SHALL return `uri`, `vmName`, `operationId` (the caller's `operationId`, or `console-<instance>` when none is given) and `status: "started"`. A session for the same instance SHALL be closed first, and its process ended; there SHALL be at most one session per instance. An instance that does not resolve, or a console that cannot start, SHALL end the call with a typed tool error and leave no process.

#### Scenario: One session per instance
- **WHEN** a client starts a console on an instance twice
- **THEN** the second call returns `status: "started"`, the first session's process has ended, and only one console process runs for the instance

### Requirement: Console output is read by pull from a bounded buffer
For each session the Rust Host Agent SHALL keep the newest 64 KiB of console output (owner decision D39). `send_console_input` SHALL write `data` to the session's terminal, wait briefly for the console to respond, and return `status: "sent"`, `operationId`, `output` (the output that arrived since the previous read, at most 64 KiB, with older output dropped) and `closed` (true once the session's process has exited). An empty `data` SHALL only read. After the process has exited, the remaining output SHALL still be returned once, with `closed: true`; a later call SHALL end with a typed tool error naming the session. An unknown `operationId` SHALL end the call with a typed tool error.

#### Scenario: Type and read
- **WHEN** a client sends a newline to a session on a booted guest
- **THEN** the result's `output` contains the guest's login prompt and `closed` is false

#### Scenario: The session ends
- **WHEN** the client sends the console's detach sequence (`ctrl+a q`)
- **THEN** a following read returns `closed: true`, the console process has exited, and a further call ends with a typed tool error

### Requirement: A console's size can be changed
The Rust Host Agent SHALL answer `resize_console` by setting the session's terminal to `width` columns and `height` rows and returning `status: "resized"` and `operationId`. Sizes that are not positive, or an unknown `operationId`, SHALL end the call with a typed tool error.

#### Scenario: Resize
- **WHEN** a client resizes a session to 120 by 40
- **THEN** the call returns `status: "resized"`

### Requirement: Sessions end with the agent
When the Rust Host Agent stops, it SHALL end every console session and its process.

#### Scenario: Shutdown
- **WHEN** the agent receives SIGTERM with a console session open
- **THEN** no console process the agent started remains
