# M12b notes: the soak, its masks, findings

## Entry

M12b branches from `main` at `40b48383`, after
wunderous/opute-host-agent#23 (D25 decided, not implemented). The Go
authority is unchanged: `wunderous/host-agents` `ace7013d` (tree
`ac17cfb2`), package `@opute/host-agent@0.2.3`. The Go reference
(`71924b62…`) and the Rust candidate (the x64 release artifact,
`e2951200…`) are the ones the m12a round pinned, and the harness hash is
unchanged. The m12a suites are therefore not rerun: the m12b gate
re-derives them from the committed m12a receipts.

## Duration (owner decisions, 2026-11-01)

The plan asked for 72 hours or more. The owners dropped that: validation 5
is a 15-minute soak, and no validation step in any milestone may run longer
than 30 minutes. `milestones.md` records both, and the inventory gap is
renamed from "72-hour soak" to "soak". The committed m12a evidence keeps its
record of that round unchanged, so `evidence/m12a/verify.py` names the old
gap; the m12a gate itself does not run that script and still passes.

## The soak

`parity soak run` boots two disposable qemu twins from the pinned base
golden (6 GiB each: `remove_host_file` is a heavy-class operation and
admission reserves 2 GiB for it). Each twin runs the same guest program
(`tools/parity/t2guest/soak.py`) with the same seeded schedule, one
iteration per second:

| Operation | Weight | Call |
| --- | --- | --- |
| `tools/list` | 12 | read-only |
| `get_host_info` | 12 | read-only |
| `opute.provider.status` | 6 | read-only |
| `ensure_host_file`, `remove_host_file` (`confirm: true`) | 10, 6 | mutation (remove is a task) |
| `ensure_sqlite_database`, `get_sqlite_database_status`, `remove_sqlite_database` (`confirm: true`) | 6, 6, 4 | mutation and read |
| input-required task, answered or cancelled | 6, 4 | `request_task_input`, then `tasks/update` or `tasks/cancel` |
| `run_host_plan` (inspect, then `get_host_info`) | 8 | plan task |
| `opute.provider.reload` (pinned Go k3s provider) | 2 | provider replacement task |

Every 60 iterations the agent restarts, alternating SIGTERM and SIGKILL.
Every 10 seconds each twin samples the agent's RSS, open FDs, threads and
state size. At the end it stops the agent and dumps the journal and every
durable row. The host mirrors each twin's files and treats a dead qemu or a
failed soak unit as a crash; a crash is recorded, never restarted.

## What is compared

`parity soak evaluate` re-derives everything from the raw receipts:

- **Outcome classes:** every iteration's class (transport, HTTP, JSON-RPC
  code, tool error code, task status, or `ok`) must equal Go's, in order.
- **Latency:** per operation, p50, p95 and p99 of the call and of call plus
  task wait, within the declared ratio of Go's plus the declared slack.
- **Growth:** RSS, FDs and threads at each restart epoch's end, as a slope
  across the run, within Go's plus the declared slack; state size growth
  within 2× Go's plus slack; no more down samples than Go's plus slack.
- **Restarts:** the same kinds, each started and ready.
- **Journal:** no more error lines than Go.
- **Final state:** `state.db` rows equal after these masks:
  - M5 shape's wall-clock masks and the supportedTools set (F-2), plus plan
    node `startedAt`/`completedAt` and provider `active_at`;
  - task ids replaced by the iteration that reported them; rows keyed by
    random ids (invocations, generations) compared in insertion order, with
    each id aliased so its other occurrences compare too;
  - M5 shape's host reservation id mask (wall-clock nanoseconds);
  - D19 (neutral host tool names and the pinned revisions) and D13 (Rust
    stores open structured leaves as `"[redacted]"`);
  - `get_host_info`'s `system` object, in stored invocations and plan state,
    masked on both sides as a live reading of each VM (see below);
  - the catalog revisions of the provider generations each reload creates,
    which are not pinned: they must pair one to one across the whole state,
    and no pinned Go revision may be among them.

## Thresholds

Declared before the soak and committed in their own commit
(`tools/parity/soak-thresholds.json`), derived by rule from the Go/Go
shakedown's raw receipts (`parity soak derive`): Rust may differ from Go by
twice the spread two identical Go twins showed, never less than the stated
floors. `verify.py` re-derives the file byte for byte from the shakedown.

## Findings

- **Provider reload needs a descriptor file.** Go's `handleProviderReload`
  checks `source` or `descriptor` as strings, so an inline descriptor object
  is refused with "provider reload requires descriptor source"; Rust does
  the same. The soak's reload names a descriptor file (`source`), which both
  read; each reload then activates a new generation and drains the old one.
- **Each reload publishes a new catalog revision.** The generation is part
  of the catalog, so every reload changes the revision on both sides. Only
  the first k3s generation's pair is pinned in the manifest, hence the
  one-to-one pairing rule above.
- **`get_host_info` stores a live snapshot.** A Go/Go trial showed the
  stored `system` object differs between two Go twins (memory, disk, PSI,
  cgroup, admission readings, and load averages left out when they read
  0.00). Go/Rust already hid its values through D13; masking it on both
  sides makes Go/Go and Go/Rust compare the same thing.
- **No difference between Go and Rust was found.** Outcome classes, restart
  behavior, journal error counts and final state matched in both shakedowns
  and in the soak (see [README.md](README.md)).

## Harness fixes during the milestone

Three smoke runs and one trial shaped the harness before any evidence
round. None of them is evidence:

1. Smoke 1: the monitor read `systemctl show` in the wrong order; pacing
   started before setup; the provider listens on `*:3015`; heavy operations
   did not fit a 2 GiB twin.
2. Smoke 2: the hand-typed recipe failed its pinned sha256 (now read
   verbatim from the postgres scenario); `remove_sqlite_database` needs
   `confirm`; random invocation ids needed alignment.
3. Smoke 3: the reservation mask, the alias rule (only random-looking keys)
   and D13.
4. Smoke 4: the reload descriptor file and the revision pairing.
5. Trial Go/Go shakedown, started by hand before `run.sh` existed: retired
   to `.parity/retired/m12b-trial-shakedown-go-go` with its cause. It
   showed the live snapshot difference.
6. Soak round 1 (seed 1203): retired to `.parity/retired/m12b-soak-round1`.
   Seven of eight checks passed. `noDownSamplesOutsideRestarts` failed
   because the code counted every down sample, restart windows included,
   contrary to its name: Rust's two down samples fell inside its own
   restarts (Rust is ready in about 0.3 s, Go in 2 s). The check now
   excludes each restart's window (`[t - readyMs, t]` plus 0.5 s of sampling
   jitter) and records the total separately. The thresholds re-derive
   unchanged, and the Go/Rust shakedown's `evaluation.json` was re-derived
   from its unchanged raw receipts. A fresh round ran under the fixed
   harness; the retired one was not relabelled.

## Clean-tree receipts

`run.sh` hashes `git status` plus `git diff HEAD` before and after each
phase. The shakedown commit tracked `checks/exits.log`, so the soak phase's
own three appended exit lines show in its `tree.after`. `verify.py` rebuilds
that exact state (the soak head plus those three lines) in a temporary
worktree and requires the recorded hash, so any other change during the
soak would still fail it. `run.sh` is kept as it ran.
