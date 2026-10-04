# M5 evidence: durable state and schema-derived redaction (in progress)

**Latest implementation refresh:** [2026-10-04-progress.md](2026-10-04-progress.md)
supersedes the status-discrepancy and unimplemented-store statements below.
It also records the restored D12 admission-before-operation contract and
distinguishes historical intermediate runs from current validation. M5 is
still incomplete.

This file records partial progress on M5 from
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md)
and task 4.3 in
[tasks.md](../../openspec/changes/reimplement-host-agent-in-rust/tasks.md).
**M5 is not done.** This README lists only what has been built and
verified so far, and what is still missing, so it can be trusted rather
than re-derived. Reproduced 2026-10-04: Rust 1.93.1, pinned Go 1.25.4,
Python 3.14 on Ubuntu-26.04 under WSL2. Every run used isolated sandboxes
and Incus shims only; no real Incus, Kubernetes, tunnel or shared state was
touched.

## What is built and verified

| Area | Status |
| --- | --- |
| `_txlock=immediate` parity | `StateStore::transaction_immediate()` added; exercised by `concurrent_writers_wait_rather_than_fail_busy` (8 threads, separate connections to one file, mirrors Go's `TestConcurrentWritersWaitRatherThanFailBusy`) |
| Operations CRUD | `create_operation`/`complete_operation`/`fail_operation`/`cancel_operation`/`list_operations`/`get_operation`, ported from `internal/state/store.go` |
| Plan CRUD | `create_plan`/`find_plan`/`get_plan`/`update_plan`/`update_plan_document_hash`/`update_plan_catalog_revision`, and the atomic `complete_plan_with_active_capability` write |
| Task snapshot persistence | `save_task_snapshot`/`list_task_snapshots`, wired into the live task lifecycle (below), not just the store layer |
| Task lifecycle wiring | `tools.rs`/`transport.rs` call `create_operation`/`complete_operation`/`cancel_operation` and persist a snapshot (`persist_task`) at every transition (create, complete, cancel, update), mirroring Go's `internal/hostmcp/server.go` call-site pattern: in-memory transition first, durable write second, errors discarded (`let _ =`, matching Go's `_ = s.state.X(...)`) so a durability failure never fails the in-memory task |
| Startup restore | `app.rs` rebuilds the in-memory task registry from `list_task_snapshots()` before the server exists (`tasks::Registry::restore_snapshot`), mirroring Go's `restoreTasks()`. A task snapshotted mid-flight (`working`) comes back `failed`, matching Go's crash-safety default — never silently resumed, never invented as `completed` |
| `toolArgs` redaction | `redact_task_args` ports Go's `redactTaskArgs` (write-only projection against the input schema, same mechanism `redact_task_result` already used for output); found missing via a live shape-parity run (below), then fixed and reverified |
| Shape-parity dump primitive | `Sandbox.sqlite_rows()` (`tools/parity/parity/agent.py`): full typed row content per table, keyed by primary key (not list order) so Go/Rust insertion-order differences don't cascade into spurious diffs; wired into the harness as a new `"sqliteRows"` collector alongside the existing `"sqlite"` (schema/rowCounts only) one |
| Live shape-parity result | `tasks.input-required.round-trip` passes cleanly: zero `state.db` diffs, one declared divergence (`D8.authz-rows`, confirmed non-stale) |
| Rust unit tests | 101 pass (`cargo test --release --locked`), `cargo fmt`/`cargo clippy -D warnings` clean |
| Older-state migration, unit-level | `open_migrates_legacy_active_runtime_columns` (`crates/host-agent/src/store.rs`) mirrors Go's own `TestOpenMigratesLegacyActiveRuntimeColumns` (`internal/state/store_test.go`) exactly: seeds a pre-column-addition `active_runtimes` table (its three original columns only, no `active_capabilities` table), opens it with `StateStore::open`, and asserts the row surfaces through `get_active_capability` with `runtime` renamed to `provider`. |
| Older-state migration, cross-implementation (**Done when** item 6 for this check) | `python3 -m parity migration --go <go> --rust <rust> --repeats 3 --out .parity/migration-check`: seeds the identical legacy `active_runtimes` row into a fresh sandbox's `state.db` *before first start* (no DB-open API exists outside the real binary, so this is the only way to exercise each binary's own startup-time migration, not a helper function), starts each real binary against its own copy, and diffs the resulting `active_capabilities`/`active_runtimes` row content. 3/3 repeats identical; inspected the raw dump directly (not just the harness's pass/fail) and confirmed both sides produce byte-for-byte the same migrated row, including every defaulted column (`recipe_id`, `run_id`, `activated_at`, etc. all `""`, matching Go's defaults exactly). New: `tools/parity/parity/migration.py`, wired as `python3 -m parity migration`. |
| Cross-read (**Done when** item 2) | `python3 -m parity cross-read --go <go> --rust <rust> --out .parity/cross-read-check`: one side runs a task to a terminal status (`start_vm`'s deterministic binding-failure completion, no shim needed) and is stopped; its state directory is copied; the *other* binary starts cold on the copy and answers `tasks/get` for the same task. Never concurrent — the writer's process has exited before the reader's ever opens the directory. Both directions (`go-writes-rust-reads`, `rust-writes-go-reads`) pass identically, 3/3 repeats, no flakiness; inspected the raw dump and confirmed the reader reports the real completed status and binding-failure result, not a vacuous match. `tasks/list` is not part of this check: confirmed it returns `Method not found` on both sides (`transport.rs`'s `"tasks/list"` branch), so there is no "list" query to cross-read yet, only "status" (`tasks/get`). New: `tools/parity/parity/cross_read.py`, wired as `python3 -m parity cross-read`. |
| X3, partial: task-arg redaction against the real catalog | `every_task_aware_tools_write_only_fields_are_redacted` (`crates/host-agent/src/tools.rs`) walks both real catalog modes (host, standalone), finds every task-aware tool with a top-level `writeOnly` input property (currently `opute.provider.install` and `opute.provider.reload`'s `token`), places a unique canary in each, and asserts `redact_by_schema` — the exact function `redact_task_args` calls before a snapshot reaches `state.db` — strips every one. This is the in-process redaction step only: it does not sweep the live sinks X3 names (WAL/SHM bytes, logs, stdout/stderr, HTTP responses), and no scenario in this harness currently drives `opute.provider.install`/`reload` to completion (both only appear in admission-*rejection* scenarios), so the live, on-disk half of X3 for this field is still unverified. |
| Parity harness tests | 83 pass (`python3 -m unittest discover -s tests` under `tools/parity`), including 3 new tests for `sqlite_rows()` (one of which caught a real bug in primary-key detection before any live run) |
| Crash-injection mechanism (`parity crash`), `"mid-shim"` mode | Kills the agent (`SIGKILL`) while `start_vm` is blocked mid-dispatch on the Incus shim gate (so `Create` + the initial snapshot have committed but `Complete`/`Cancel` have not), restarts the same binary against the same state dir, and diffs recovered state against the other side. **200/200 seeds pass identically** on both Go and Rust (scaled up from an initial 25; see the open-items section below for what "200 seeds at one fixed injection point" does and doesn't prove): `tasks/get` after restart reports `"failed"`/`"The Host Agent restarted before the task completed."` on both sides (exact text match), `operations.status` becomes `unknown` via the open-time migration on both sides, and the stale pre-crash `task_snapshot_json` is left untouched on both sides (restore only rebuilds the in-memory registry; it does not re-persist, matching Go) |

## New divergence declared

`D8.authz-rows` (`tools/parity/divergences.json`): the `sqliteRows` full-content
analog of the already-existing `D8.authz-row-counts` decision. Every scenario
that starts the standalone server also provisions `authz.sqlite`'s built-in
clients and oauth-issuance tables (D8), so any scenario collecting
`sqliteRows` needs this declared or it fails on content that's already an
approved, out-of-scope difference. Confirmed non-stale (the dropped content
actually differs between sides, not leftover dead config).

## Scope changes and open items

- **Full M1–M4 corpus not yet run.** Only `tasks.input-required.round-trip`
  passes cleanly end to end. `sqliteRows` collection was also added to
  `tasks.cancel.before-start`, `tasks.cancel.mid-shim`,
  `tasks.cancel.after-completion` and `tasks.async.binding-failure` (see
  below); the admission-matrix scenarios that create many concurrent,
  uncaptured task ids per run were deliberately not attempted yet, since
  `sqliteRows` keys rows by primary key and a scenario with several
  uncaptured random task ids would need per-row identity alignment this
  harness doesn't do yet, not simple mask/substitution reuse.
- **`operations.status` discrepancy: root cause now confirmed. Not a race.**
  Earlier entries in this file treated this as a timing-dependent race
  between two uncoordinated writers and recommended root-cause work or a
  defensive transaction as open options. That was wrong in a specific way:
  reading Go's source end to end finds two distinct, fully deterministic
  bugs, reproducible on every run, not a race at all.
  1. **Go's `SaveTaskSnapshot` (`internal/state/store.go:759-781`) silently
     resets `operations.status` to `"working"` on almost every call.** It
     computes the status to write with `if candidate, ok :=
     value["status"].(string); ok { status = candidate }` — but
     `value["status"]` is always a `tasks.Status` (`type Status string`,
     `internal/tasks/registry.go:18`), not a plain Go `string`. A Go type
     assertion requires the *exact* dynamic type, so `.(string)` against a
     named string type always fails; `ok` is always `false`, and `status`
     keeps its hardcoded default, `"working"`. Every `persistTask` call
     that is the *last* write to a row — true for `tasks/cancel`'s single
     `Cancel()` → `persistTask(rec)` sequence — leaves `operations.status`
     at `"working"` **regardless of the task's real status**, while the
     embedded `task_snapshot_json` and `result_json` are correct. Rust's
     `save_task_snapshot` (`store.rs:342`) extracts status with
     `snapshot.get("status").and_then(J::as_str)` against a real
     `serde_json::Value`, which has no such type-identity pitfall — it
     reads the true status every time. **This is the actual explanation**
     for every `tasks.cancel.*`/`tasks.async.binding-failure` diff seen all
     session (Go stuck at `working` or `unknown`-via-migration, Rust
     showing the real `cancelled`/`completed`): Go has a bug, Rust avoided
     it by construction, and that avoidance is itself an undeclared
     deviation under "parity by default."
  2. **A second, separate bug**, present in **both** Go and Rust (so it
     does not show up as a cross-implementation diff, but is real):
     `tasks/update`'s handler persists a *stale* pre-resume snapshot after
     the resume already ran and already persisted completion. Go's
     `Registry.Update` (`internal/tasks/registry.go:356-391`) takes
     `snapshot := cloneRecord(rec)` *before* calling `resume(accepted)`
     synchronously, then returns that stale snapshot; the `tasks/update`
     handler (`internal/hostmcp/server.go:1994`) then calls
     `s.persistTask(updated)` on it — overwriting the correct
     `"completed"` row that `resume()`'s own `Complete()` +
     `persistTask(completed)` had just written. Rust's `Registry::update`
     (`tasks.rs:316-344`) is a line-for-line port of the same ordering,
     down to the same stale-clone-before-resume pattern, and
     `transport.rs`'s `"tasks/update"` handler persists it the same way.
     This is why `tasks.input-required.round-trip` passes *cleanly*: both
     sides end at `operations.status = "working"` after a successful
     resume, matching each other — not because the state is correct (it
     isn't: the real status is `"completed"`, visible only in `result_json`
     and the live in-memory task), but because both implementations carry
     the identical bug. Verified directly against the raw row: `result_json`
     holds the real completion payload while `status` and
     `task_snapshot_json` show the stale pre-resume `"working"` snapshot —
     see `.parity/full-recheck2/scenarios/tasks.input-required.round-trip/`.
  Net effect: bug 2 is already faithfully reproduced in Rust (no divergence
  needed, "parity by default" is already satisfied there, however buggy).
  Bug 1 is the real, confirmed, deterministic source of every live
  `operations.status` diff this session found. It still needs the same
  owner decision the earlier entries called for, but the decision is now
  concrete rather than open-ended: either declare a D-numbered divergence
  for Rust's (more correct) behavior, or deliberately reproduce Go's type-
  assertion bug in Rust for strict bit-for-bit parity. No divergence has
  been declared and no code has changed on either side.
  **The `release-race` crash-injection data (15 seeds, 12 failures,
  recorded below) is consistent with this root cause**: every failure is
  explained by bug 1, by relative kill-timing against the open-time
  migration, or by both — no third mechanism.
- **Crash injection: the `"mid-shim"` mode is now at the milestone's named
  scale, but at one injection point, not many.** `tools/parity/parity/crash.py`
  supports two modes. `"mid-shim"` kills while still blocked on the Incus
  shim gate, before `Complete` is reached at all — a single, fixed point,
  never varying where the kill lands relative to the completion write (and,
  given the root cause below, never able to land inside the buggy
  `SaveTaskSnapshot` write at all). Scaled from the earlier 25 seeds to
  **200/200 recovered identically** (verified: 200 distinct per-seed
  results, zero failures, `.parity/crash-mid-shim-200/summary.json`). This
  satisfies the "~200 randomized seeds" count the milestone names, but not
  its intent of varying *where* the kill lands — every one of these 200
  seeds hits the same lifecycle stage. `"release-race"` (the mode that
  varies the injection point) stays at 15 seeds, 3/15 identical; now that
  the root cause is confirmed (see below), the 12 failures are fully
  explained by Go's `SaveTaskSnapshot` bug and open-time-migration timing,
  not an unknown third mechanism, so scaling `release-race` further adds
  confirmation, not new information. Adding injection points at other
  lifecycle stages (mid-plan-write, mid-cancel) is still open.
- **X3 is started, not green.** The in-process redaction check above covers
  only the two task-aware tools whose catalog schema declares a top-level
  `writeOnly` property, and only the redaction function, not a live sink
  sweep. It does not cover: write-only fields nested inside a free-form
  `inputs` object (the recipe-runner tools accept secrets this way and the
  schema can't statically see them), the five non-task-aware tools with
  `writeOnly` fields (`ensure_host_file`, `ensure_public_mcp_tunnel`,
  `install_cluster_agent`, `probe_openai_compatible_server`,
  `put_k8s_secret`, `run_instance_command` — not persisted today, so lower
  priority, but unverified), or any of the live sinks X3 actually names
  (grep across `state.db`/`-wal`/`-shm`, logs, stdout/stderr, HTTP
  responses). Reaching the live sinks for `opute.provider.install`/`reload`
  needs a scenario that drives either tool to completion, which does not
  exist in this harness yet (both appear only in admission-rejection
  scenarios) — building one is its own piece of work, not attempted here.
- **Not started**: `provider_generations` and `capability_invocations` CRUD,
  lifting the schema-derived redaction mechanism out of `tools.rs` into
  something shared, and wiring Go's plan-evidence redaction (`redactPlanEvidence`)
  once a plan executor exists (M6).
- **Unknown-projection-must-fail-closed: verified by source comparison, not
  by a new runtime test.** Read both sides rather than running one, since
  building a full in-process `Server` fixture just to exercise this guard
  clause would be disproportionate to what it's checking. Go's
  `redactEvidenceBySchema` (`.parity/go-src/internal/hostmcp/evidence_redaction.go:42`)
  does *not* fail closed at the leaf level — a key with no matching
  `properties`/`additionalProperties` entry passes through raw
  (`default: return value`), and Rust's `redact_by_schema` matches that
  exactly (`other => other.clone()`, `tools.rs:385`). The actual fail-closed
  behavior this requirement means is one level up: an entire evidence blob
  for a tool/capability/node `redactEvidenceBySchema` has no schema for at
  all becomes `{"redacted": true}` wholesale — see Go's
  `redactPlanAction`'s "capability not found" branch
  (`evidence_redaction.go:144-147`) and `redactPlanRunState`'s "no node
  schema" branch (`evidence_redaction.go:225-233`). Rust's `redact_task_args`
  and `redact_task_result` (`tools.rs:330`, `tools.rs:344`) already
  have the identical guard — `let Some(descriptor) = ... else { return
  json!({"redacted": true}) }` — for an unrecognized tool name, added
  earlier this session alongside `redact_task_args` itself, before this
  requirement was checked against Go's source. `redactPlanEvidence` (whole
  plan, not a single tool call) isn't portable yet since no plan executor
  exists (M6), so that specific fail-closed path remains unverified.
- **`D8.authz-rows` needs no new OpenSpec change or contract suite**:
  contract suites are keyed by decision number, not by individual
  `divergences.json` path entries (confirmed against `D11`/`D12`, each a
  single suite covering several dropped-row-count paths), and `D8` already
  has both — `openspec/changes/secure-oauth-issuance/` and
  `tools/parity/contracts/oauth-issuance.json`, with canary `K6` (
  `credentials.first-start`) already exercising the same built-in-client
  provisioning content `D8.authz-rows` drops. Correcting the stronger claim
  this file made earlier. **No OpenSpec change or contract suite exists
  yet** for the undecided `operations.status`/atomicity finding above,
  since no divergence number has been assigned to it.

## Done when (not yet met)

None of the six M5 "Done when" criteria in milestones.md are satisfied yet.
This file will be rewritten, not appended to, once they are — it should
never claim more than the evidence in `evidence/current/` and this
directory actually proves.
