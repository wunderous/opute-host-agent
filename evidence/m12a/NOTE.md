# M12a notes: entry, Go's cutover behavior, findings

## Entry

M12a branches from `main` at `bd239c76`, after
wunderous/opute-host-agent#21 merged. The Go authority is
`wunderous/host-agents` `ace7013d` (tree `ac17cfb2`), package
`@opute/host-agent@0.2.3`.

M12 formally depends on M11, and M11 needs staging Platform. This round runs
only the M12 items that need no Platform, and records M11 as the open
dependency.

## Go's cutover behavior (inventoried before the harness)

- **Shutdown (`internal/app/app.go`):** on SIGINT or SIGTERM, Go calls
  `httpSrv.Shutdown(context.Background())`, which has no deadline:
  - the listener closes;
  - idle connections close (a connection that has sent nothing counts as
    idle once it is 5 s old);
  - in-flight requests run to completion, and their replies carry
    `Connection: close`.

  Then the deferred closes run. `Server.Close` cancels every working task
  and persists it as `cancelled`, "The Host Agent shut down before the task
  completed." (`closePendingAsyncTaskStarts`). It then waits for the plan
  goroutines, so a task blocked in a host command holds the process until
  that command returns. The exit code is 0.
- **Task restore (`internal/tasks/registry.go` `RestoreSnapshot`):** a
  `working` snapshot restores as `failed`, "The Host Agent restarted before
  the task completed."; the other statuses are kept. A task resumed through
  `tasks/update` can restore as failed from its stale working snapshot (the
  M5 cross-read quirk). Go's own restart is therefore the oracle for "Go's
  answers after a restart".
- **State-directory lock:** none. A second agent started on the same state
  directory starts and serves. Each agent sees only its own in-memory
  tasks; a later restart sees both agents' persisted tasks.
- **Operation records:** Go writes a task's operation row at task creation,
  before resource binding (`internal/hostmcp/server.go:1642-1647`). Under D12,
  Rust writes it only after admission.
- **`/health`:** lists Incus itself (`incus list --format json`). The harness
  must account for that when a shim gate is involved.

## Rust change M12a needed

The in-flight drill found one parity bug. On SIGINT, Rust closed its task
starts and providers and returned at once. An in-flight synchronous call got
no reply, while Go answers it (200, `Connection: close`) and exits 0.

`app.rs` and `http1.rs` now shut down as `http.Server.Shutdown` does:

- the listener is dropped;
- connections waiting for a request close at once;
- a connection that has sent nothing closes at age 5 s;
- in-flight requests finish, with keep-alives off;
- then task starts and providers close.

Unit tests cover the three connection cases. A raw-socket probe against both
binaries matched header order, timing and exit code.

## Rehearsal design (`parity rehearsal`)

- **Origin:** Go builds standalone state under one explicit identity
  (`parity-m12a-agent`):
  - A: waiting for input;
  - B: cancelled;
  - C: resumed through `tasks/update`;
  - D: refused at admission (foreign tenant), completed with a
    `resource_binding` error;
  - E: `start_vm` blocked on a shim gate inside resource binding.

  Go is told to stop with E in flight. The harness checks that Go waits,
  refuses new connections, and exits 0 once the shim returns. The stopped
  state is copied to a snapshot and every file is checksummed.
- **Phases:** each runs on its own copy, so the snapshot is never written,
  and its checksums are compared at the end.
  - **Reference:** Go restarts on a copy.
  - **Forward:** the target (Go as the control, or Rust) starts, answers,
    restarts and answers again. It then accepts work of its own: a new
    waiting task G, and cancelling A.
  - **Rollback:** Go starts on the target's copy.
- **Checks:** per rehearsal, for each target:
  - snapshot unchanged;
  - origin really in flight at stop;
  - forward equals Go's restart;
  - stable across the target's restart;
  - rollback keeps every task with the status and message the target last
    reported;
  - identity kept;
  - clean stops.

  The Go → Go → Go control is then compared with Go → Rust → Go.
- **Drills:**
  - A synchronous `list_vms` blocked mid-shim at SIGINT.
  - Long tasks at SIGINT:
    - admitted: `start_vm` on an owned container blocked in
      `incus start`, using fixture `incus-gated-start.json`, the M8b estate
      with one gate;
    - binding: blocked in `incus list`.
  - Two agents on one state directory.
- **Safety:** the copies run without shims. The driver refuses to run if any
  `incus` exists on `/usr/bin:/bin`, the only PATH entries after the
  sandbox's own. Nothing outside the sandboxes is touched.

## Findings

1. **No converter is needed.** Rust starts on Go's stopped state as is. Its
   answers before and after its own restart equal Go's restart on a copy, in
   all three rehearsals. This matches the M5 recommendation and cross-read.
   The rehearsal state is representative, built by Go in the harness; no
   real host state was copied.
2. **The writer lock asked for is not Go behavior.** M12 validation 4 and
   task 4.4 ask that two writers on one state directory be prevented, and
   the goal asked that the second agent refuse on both sides. The pinned Go
   agent takes no lock, so a refusal would be new policy. The rehearsal
   records parity instead: both implementations start the second agent and
   behave identically. The lock stays an owned gap (`core`); it needs its
   own OpenSpec change and owner decision. The rehearsals keep one writer by
   procedure: each agent is stopped before the next starts on that copy.
3. **The in-flight policy is Go's drain.** At SIGINT:
   - in-flight requests finish;
   - working tasks are persisted as cancelled;
   - the agent waits for their host commands, then exits 0.

   Rust now matches this in every drill.
4. **D12 shows at shutdown.** A task still in resource binding when the
   agent stops has an operation row on Go, which restores it cancelled. Rust
   has nothing durable for it, so after a restart its handle is unknown
   (-32603, as for any unknown task). This follows from owner decision D12:
   persist an operation only after admission. It is declared with per-side
   expectations, and the rest of that drill is compared.
5. **arm64 stays owned.** No `qemu-system-aarch64` or user-mode emulation is
   installed, and the sandbox cannot install packages. Asked whether to
   install one, the owner chose to keep the gap owned (2026-10-24).

## Declared normalizations

| Where | What | Why |
| --- | --- | --- |
| Rehearsal answers | Task ids → `${TASK_X}`; `createdAt` and `lastUpdatedAt` → `<rfc3339>` when they have that format (any other value is reported) | Per-run values |
| Rehearsal stops | Stop durations dropped; exit status and `killed` compared | Machine timing |
| Rollback check | Status and message per task | Go's own restart drops the pending `inputRequests` of a task cancelled before it, on either side; the full documents are compared Go control against Rust |
| `longTask.binding` | `afterRestart` held to per-side expectations (Go: cancelled; Rust: unknown task) | D12 |
| Boot journals (dist) | `journald-stream-order`: an agent line recorded after systemd's exit lines in the same start block moves back before them | journald orders a service's stream and PID 1's messages independently; the agent cannot log after it exited (round 1) |

The M10 normalizations are unchanged ([../m10/NOTE.md](../m10/NOTE.md#declared-normalizations)).

## Harness and manifest fixes during the milestone

- **Round 1** was retired under `.parity/retired/m12a-round1` with its
  cause. Every step passed except `dist`, on the journald ordering race
  above. The harness fix was committed before round 2, and round 2 reran
  the whole script.
- **The Rust candidate pin.** `make parity-manifest` ran after the release
  set was re-pinned and synced `rustCandidate` from the host build in
  `target/release`. The gate reports therefore found every Rust capture
  "not the recorded candidate". The pin was corrected in its own commit to
  the release artifact the round ran against
  (`checks/binaries*.sha256`). No capture changed.
- **The M5 shape threshold.** The shape verifier still required five
  repeats. It now requires three, like every other suite (minRepeat 3). It
  only reads evidence.
- **Docs edited during round 1.** Doc edits were briefly in the worktree
  during round 1, while harness, go-reference, fmt and clippy ran. They were
  moved out at once. Round 1 is retired anyway; round 2 ran on a clean tree
  (`checks/tree.*.sha256`).

## Repeat counts

Every suite repeats 3 times: T1 Go/Go and Go/Rust, T2 both suites, M5
shape and migration, and the rehearsals.
