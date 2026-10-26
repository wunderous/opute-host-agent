## 1. Review

- [ ] 1.1 Obtain the owners' decision on D25: A (lock in Go and Rust), B (Rust only) or C (no lock, one writer by procedure).
- [ ] 1.2 Under A or B, decide the lock file name and place, the refusal exit code and stderr line, the systemd restart behaviour for a refused unit, and the scope (modes, native Windows, filesystems).

## 2. Implementation (only after 1.1)

- [ ] 2.1 Under A: change Go in `wunderous/host-agents` and cut a release; rebase the parity inventory onto it.
- [ ] 2.2 Under A or B: change Rust to take the lock before opening stores and binding the listener.
- [ ] 2.3 Under C: amend M12 validation 4 and task 4.4 to one writer by procedure.

## 3. Evidence

- [ ] 3.1 Change the M12a `twoWriters` drill to the decided behaviour: under A, Go/Go, Rust/Rust, Go-then-Rust and Rust-then-Go all refuse identically; under B, per-side expectations; under C, unchanged.
- [ ] 3.2 Add a SIGKILL case: the next agent starts.
- [ ] 3.3 Under A or B, add a refused-second-unit case to the clean-machine install and upgrade journeys.
