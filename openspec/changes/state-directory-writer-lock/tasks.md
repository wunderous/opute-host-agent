## 1. Review

- [x] 1.1 Obtain the owners' decision on D25: A (lock in Go and Rust), B (Rust only) or C (no lock, one writer by procedure). Decided 2026-11-01: option A.
- [x] 1.2 Confirm the details proposed in [design.md](design.md): the lock file name and place, the refusal stderr line and exit code, the systemd units, `check`, and the scope (modes, native Windows, filesystems). Confirmed by the owners on 2026-11-01, as written.

## 2. Go first (`wunderous/host-agents`)

- [x] 2.1 Add the lock in a small package with Unix (`flock`) and Windows (`LockFileEx`) files, and take it in `app.Run` after `validateConfig` and before `buildHostRuntime`, as design.md places it. Done in wunderous/host-agents#28, merged as `29103271`: `internal/instancelock`, taken in `app.NewRuntime` and `app.Check`, released in `Runtime.Close`.
- [x] 2.2 Add Go tests: a second `Run` on the same instance refuses with the decided line and exit 1 and writes nothing; a SIGKILLed holder leaves no stale lock; a state directory outside the instance root is covered; `check` follows the decided rule. Done in host-agents#28 (`internal/instancelock/lock_test.go`, `internal/cli/instance_lock_test.go`). gofmt, `go vet ./...` (plus Windows vet), `go test ./...` and `make test-all-modules` pass with go1.25.4, run once, and host-agents CI is green.
- [x] 2.3 Review the change with the Go owners, merge it and cut a release through Go's normal release path. Released in Go v0.2.4 (`c9f9e775`), the GitHub release the parity rebase pins.

## 3. Parity rebase, then Rust

- [x] 3.1 Rebase the parity inventory onto the Go release (source lock, Go reference, catalogs, reruns of the affected milestones), as AGENTS.md requires for a newer Go release. Pinned to v0.2.4; every suite the gates re-derive was rerun in [`evidence/d25`](../../../evidence/d25/run.sh) (T2 Go/Go in seven shards under the 30-minute limit, the 15-minute soak included), and gates m0–m6, m8a–m8f, m10, m12a, m12b and d25 pass.
- [x] 3.2 Change Rust to take the same lock at the same point in start-up, with the same file, line and exit code, and the same tests. Done in `032fe6cd8`; Go's own lock tests run as owner tests in the round (`go-owner-lock`). D28 (2026-11-08): `a_second_runtime_on_held_state_is_refused_and_writes_nothing` adds the runtime-level check, a refused second start leaving every file under the root byte-identical (shown red by moving the lock after the store open).

## 4. Evidence

- [x] 4.1 Change the M12a `twoWriters` drill: Go/Go, Rust/Rust, Go then Rust and Rust then Go all refuse identically, and the first agent keeps serving. The rehearsal's `writerLock.go-go`, `rust-rust`, `go-rust` and `rust-go` drills pass in evidence/d25 (the d25 gate requires each).
- [x] 4.2 Add a SIGKILL case: the next agent starts. Part of each writer-lock drill.
- [x] 4.3 Add a refused-second-agent case to the clean-machine install and upgrade journeys: the user unit and the bootstrap unit on one instance. The dist journeys on Ubuntu 24.04 and Debian 12 refuse the user unit while the bootstrap unit serves and start it once the bootstrap unit stops.
- [x] 4.4 Close the `state-directory writer lock` gap in the inventory only when 4.1–4.3 pass on both sides. Closed in `0469584ea`; the d25 gate requires the item covered.
