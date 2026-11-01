## ADDED Requirements

### Requirement: One running agent per instance state
Under decision D25 (option A), a Host Agent, Go or Rust, SHALL take an exclusive, non-blocking lock on its instance's lock file before it opens any durable store or binds its listener. It SHALL hold the lock until the process exits. Until the Go release that carries the lock is pinned, the pinned Go behaviour (no lock) stands, and the Rust agent SHALL keep it.

#### Scenario: A second agent on the same instance root
- **WHEN** an agent is running and a second agent starts with the same instance root
- **THEN** the second agent exits non-zero before serving, prints the decided stderr line naming the lock file, and writes nothing to the stores, while the first agent keeps serving unchanged

#### Scenario: A crashed agent leaves no stale lock
- **WHEN** an agent holding the lock is killed with SIGKILL
- **THEN** the next agent on that instance root starts normally

### Requirement: Go and Rust refuse each other identically
The refusal SHALL be the same observable contract on both implementations: exit code, stderr line, nothing written. A Go agent SHALL refuse to start on an instance root that a running Rust agent holds, and a Rust agent SHALL refuse one that a running Go agent holds.

#### Scenario: Rollback while the other implementation is running
- **WHEN** a Go agent is started on an instance root that a running Rust agent holds, or the reverse
- **THEN** the new agent refuses exactly as it refuses one of its own kind, and the running agent's accepted operations are untouched

### Requirement: Go leads, Rust follows the pinned release
The lock SHALL land in Go first, in `wunderous/host-agents`, as a release. Rust SHALL add it only after the parity inventory is rebased onto that release, so that Go stays the authority for its exact contract.

#### Scenario: Before the Go release is pinned
- **WHEN** a second agent starts on a running agent's instance root while the pinned Go tree has no lock
- **THEN** both implementations start it, as the pinned Go agent does, and the M12a `twoWriters` drill records that parity
