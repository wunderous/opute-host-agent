## ADDED Requirements

### Requirement: One running agent per instance state
Under option A of decision D25, a Host Agent, Go or Rust, SHALL take an exclusive, non-blocking lock on its instance's lock file before it opens any durable store or binds its listener. It SHALL hold the lock until the process exits. Until the owners decide D25, the pinned Go behaviour (no lock) stands, and the Rust agent SHALL keep it.

#### Scenario: A second agent on the same instance root
- **WHEN** an agent is running and a second agent starts with the same instance root
- **THEN** the second agent exits non-zero before serving, prints the decided stderr line naming the lock file, and writes nothing to the stores, while the first agent keeps serving unchanged

#### Scenario: A crashed agent leaves no stale lock
- **WHEN** an agent holding the lock is killed with SIGKILL
- **THEN** the next agent on that instance root starts normally

### Requirement: Go and Rust refuse each other identically
Under option A, the refusal SHALL be the same observable contract on both implementations: exit code, stderr line, nothing written. A Go agent SHALL refuse to start on an instance root that a running Rust agent holds, and a Rust agent SHALL refuse one that a running Go agent holds.

#### Scenario: Rollback while the other implementation is running
- **WHEN** a Go agent is started on an instance root that a running Rust agent holds, or the reverse
- **THEN** the new agent refuses exactly as it refuses one of its own kind, and the running agent's accepted operations are untouched

### Requirement: The decision is recorded before any implementation
No lock, refusal message, exit code or lock file SHALL be added to either implementation before D25 is decided. Under option C, M12 validation 4 and task 4.4 SHALL be amended to require one writer by procedure, as the M12a rehearsal enforces.

#### Scenario: Before the decision
- **WHEN** a second agent starts on a running agent's instance root before D25 is decided
- **THEN** both implementations start it, as the pinned Go agent does, and the M12a `twoWriters` drill records that parity
