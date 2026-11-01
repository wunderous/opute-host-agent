## 1. Review

- [x] 1.1 Obtain the owners' decision on D25: A (lock in Go and Rust), B (Rust only) or C (no lock, one writer by procedure). Decided 2026-11-01: option A.
- [ ] 1.2 Confirm the details proposed in [design.md](design.md): the lock file name and place, the refusal stderr line and exit code, the systemd units, `check`, and the scope (modes, native Windows, filesystems).

## 2. Go first (`wunderous/host-agents`)

- [ ] 2.1 Add the lock in a small package with Unix (`flock`) and Windows (`LockFileEx`) files, and take it in `app.Run` after `validateConfig` and before `buildHostRuntime`, as design.md places it.
- [ ] 2.2 Add Go tests: a second `Run` on the same instance refuses with the decided line and exit 1 and writes nothing; a SIGKILLed holder leaves no stale lock; a state directory outside the instance root is covered; `check` follows the decided rule.
- [ ] 2.3 Review the change with the Go owners, merge it and cut a release through Go's normal release path.

## 3. Parity rebase, then Rust

- [ ] 3.1 Rebase the parity inventory onto the Go release (source lock, Go reference, catalogs, reruns of the affected milestones), as AGENTS.md requires for a newer Go release.
- [ ] 3.2 Change Rust to take the same lock at the same point in start-up, with the same file, line and exit code, and the same tests.

## 4. Evidence

- [ ] 4.1 Change the M12a `twoWriters` drill: Go/Go, Rust/Rust, Go then Rust and Rust then Go all refuse identically, and the first agent keeps serving.
- [ ] 4.2 Add a SIGKILL case: the next agent starts.
- [ ] 4.3 Add a refused-second-agent case to the clean-machine install and upgrade journeys: the user unit and the bootstrap unit on one instance.
- [ ] 4.4 Close the `state-directory writer lock` gap in the inventory only when 4.1–4.3 pass on both sides.
