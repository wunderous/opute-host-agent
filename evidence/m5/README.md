# M5: durable state and schema-derived redaction

**Complete. `make parity-verify-m5` reports `gate m5: PASS`.** D13 is decided
and implemented (owner, 2026-10-04): the unmarked-field projection is split
into two named policies instead of one function with a mode flag --
`redact_for_delivery` (Go's original behavior: only `writeOnly` is ever
hidden from a caller's own result, synchronous or via `tasks/get`) and
`redact_for_storage` (D13's stricter, fail-closed default: a durable audit
row or a stored echo of the caller's own arguments keeps only
schema-covered content). See
[`redact-unmarked-projections`](../../openspec/changes/redact-unmarked-projections/proposal.md)
and its `evidence-redaction` contract suite (4/4 scenarios passing, Rust
canary K24 confirming the fix is load-bearing). Both projections are proven
independently by unit test (`tools.rs`), not by inspection.

All five M5 evidence categories were re-run against the final two-projection
binary and are current (same `rustBinarySha256`, `changedDuringRun: false`):
shape 420/420 (5 repeats, 0 stale divergences), cross-read 18/18, storage-crash
200/200 seeds, secret sweep 0 failures across 16 marked fields, older-state
migration 6/6. `D13.redacted-structured-content` is declared in
`tools/parity/divergences.json`, approved in `parity-manifest.json`, and
applied (non-stale) everywhere it is exercised.

Five bugs in the parity harness itself, not in the Rust implementation, were
found and fixed while re-validating this change:

1. `tools/parity/parity/shape.py`'s `job()` re-acquired `runner._EXCLUSIVE` --
   a plain, non-reentrant `threading.Lock` -- around a call to `runner.execute()`
   that already acquires the same lock internally for fixed-port scenarios.
   The second acquisition on the same thread was a guaranteed self-deadlock,
   silently hanging the full shape matrix with zero progress after the first
   scenario or so. Fixed by removing the redundant outer acquisition.
2. `tools/parity/parity/canon.py` applies scenario masks (for live, volatile
   values) strictly before divergences ever see the combined pair. A field
   that is both scenario-masked and newly D13-redacted produced a permanent
   `$maskViolation` that no later-declared divergence could clear, and
   `capability_invocations` rows are keyed by random UUIDs that never align
   between Go and Rust until a later pass masks the id and sorts rows into
   comparable list positions -- so D13, which reaches inside each row's own
   structured content, has to run in a second pass after that conversion, not
   alongside the table/row-identity rules (D8/D11/D12) that are unaffected by
   the ordering. Fixed by making masks redaction-aware and splitting
   `shape.py`'s divergence application into an early and a late pass.
3. The same D13 effect surfaced in `tools/parity/parity/secret_sweep.py`: the
   forward-looking plan-document/run-state storage projections (M6/M7,
   currently `#[allow(dead_code)]`) also route through `redact_for_storage`
   now, so an untyped test-fixture field like `visible` or `marker` --
   unrelated to any `writeOnly`-marked secret -- comes back redacted on the
   Rust side only. Fixed the same way as `shape.py`: the paired
   `dropValueDeep` walker converges both sides on the declared divergence
   before the comparison runs, and a stale-declaration check was added
   alongside it.
4. Declaring `D13.redacted-structured-content` (and, transiently while
   chasing this, `D12.refused-task-operation-count`) directly in a scenario's
   own `compare.divergences` is only correct for the handful of scenarios
   that already collect `sqlite`/`sqliteRows` themselves. Every other
   scenario's base collection is `trace`-only, so the same declaration is
   simultaneously *required* once `shape.py` force-extends that scenario's
   collection to include `sqliteRows`, and *stale* in the regular (non-shape)
   go-vs-rust comparison, which never collects that path at all -- the two
   drivers can't be satisfied by one static list. Fixed by making `shape.py`
   auto-detect both rules dynamically (probe the normalized documents, keep
   the rule only when it finds a real, non-stale difference), exactly like
   the existing `D8.authz-*` rules already did; no scenario declares either
   rule directly for this reason anymore.
5. `tools/parity/parity/canon.py`'s `substitute()` replaced a numeric literal
   (a port) only where it wasn't flanked by another digit, to avoid
   corrupting a longer number such as a byte count. A lowercase hex hash
   (`authz.sqlite`'s `token_hash`) can coincidentally contain the port's
   exact digit run flanked by `a`-`f` letters instead of digits, which the
   digit-only boundary didn't catch, corrupting the hash on whichever side
   happened to draw that port. Fixed by widening the boundary to exclude any
   alphanumeric neighbor, with a regression test (`test_canon.py`).

The migration preserves the pinned Go revision in `baseline/source-lock.json`.
Validation uses distinct identities, credentials, ports and state directories.
No shared Incus, Kubernetes, tunnel or deployment state has been changed.

## Last completed evidence and current refresh

| M5 validation | Current status |
| --- | --- |
| Full database shape across M1-M4 | **PASS**: 420/420 comparisons (84 scenarios x 5 repeats), 0 stale divergences, `changedDuringRun: false`. The earlier 418/420 port-collision failures (two default-port cases observing another suite's listener) and the `shape.py` self-deadlock that initially blocked this re-run are both fixed; see the harness-bug notes above. |
| Cross-read | **PASS**: 18 checks in both directions against the current candidate; pending/cancelled/resumed input tasks and full durable rows |
| Varied crash recovery | **PASS, refreshed**: 200/200 seeds recovered identically. `storage-crash` SIGKILLs inside actual SQLite writes for 8 named checkpoints (operation create/complete/fail/cancel, task-snapshot, plan-update, plan-complete at two points) at BEFORE and AFTER each. Supersedes the earlier refused-task-fixture runs, which are historical only. |
| Secret-canary sink sweep | **PASS, refreshed**: 16 write-only fields swept across standalone and platform modes, zero occurrences in any live sink. The D13 storage projection now also reaches the plan/state documents this sweep inspects; `secret_sweep.py` was taught the same declared-divergence convergence as `shape.py` (see harness-bug notes above). |
| Unknown projection | **PASS (D13 implemented).** `redact_for_storage` now redacts every unmarked/open-schema value before it reaches `capability_invocations` or a task snapshot's `toolArgs`; Go's pinned, unchanged verbatim persistence is the expected, declared-divergent baseline, not a failure. `redact_for_delivery` leaves what a caller actually receives untouched -- verified directly: the live `get_vm_info` response still carries the real `state.incusStatus` value. Unknown capabilities are still rejected without durable rows. See [the owner decision record](unknown-projection/owner-decision.md) for the two options weighed, and [`redact-unmarked-projections`](../../openspec/changes/redact-unmarked-projections/proposal.md) for the implemented design. |
| Older-state migration | **PASS, refreshed**: six checks against the current candidate using the captured v0.2.2 release schema and an explicit pre-column-addition fixture |

The cross-read, migration, crash and secrets evidence bundles are under
`cross-read/`, `migration/`, `crash/` and `secrets/`. The verifier rehashes
their raw observations, checks source, binary, harness, driver and fixture
provenance, and recomputes differences and preservation assertions. Missing
directions, equal data loss, wrong restored states, changed identities,
invented success and stale artifacts cannot pass.
See [migration/README.md](migration/README.md) for release capture and reproduction.

`make parity-verify-m5` reports `gate m5: PASS` against this evidence. Cutover
requires M5 as well as its other whole-agent gates.

## Implemented

- Operations and plan storage, immediate transactions, atomic plan completion
  with active capability selection, and older-column migrations.
- Provider-generation save/list and invocation insert, preserving Go validation,
  conflict behavior, defaults and timestamps. Accepted built-in calls now record
  schema-projected invocation envelopes, execution bindings and observations.
  Arbitrary MCP result text is omitted. Schema-invalid calls retain D11's
  reject-without-audit behavior. Provider lifecycle integration remains open.
- Task snapshot persistence and restoration, shared schema projection, and
  fail-closed startup on snapshot read/decode errors.
- Accepted async workers drain before durable stores close. D12 admission
  refusals create no operation row, including cancellation and final snapshots.

Two pinned Go behaviors are retained explicitly: named `tasks.Status` values
fall back to `working` in the legacy operation-row projection, while snapshots
retain the actual task status; and successful operator resume can persist the
stale pre-resume snapshot, causing the task to restore as failed. Correcting
these behaviors needs a separate contract decision. Rust does not invent a
successful resumed task after restart.

## Checks and limitations

The current harness has **133 passing tests** (plus 9 subtests), including
migration, cross-read and shape negative controls and the D13 divergence
coverage added for this change. The latest Rust implementation passed 106
unit tests (2 ignored by design -- the isolated M5 secret-canary driver,
exercised separately by the harness), formatting, clippy with warnings
denied and a release build. Strict OpenSpec validation passed all six items.

The secret sweep retains raw bytes and hashes for every scanned state file,
including live SQLite WAL and SHM, and independently recomputes canary absence.
It requires each task snapshot, invocation audit, plan document/state and active
selection to exist and contain the expected projection. Normal agent processes
restore the tasks and expose completed result/status through `tasks/get`.
Pinned `tasks/list` and `tasks/result` are unsupported; their captured refusals
are checked explicitly and do not prove an implemented event API.
Projection/storage fixtures execute no provider effects and do not prove the
M6 executor or M7 provider lifecycle.

Full-row discovery exposed missing accepted invocation audits in Rust; that
runtime path is now implemented. The shape driver reuses each scenario's typed
live-measurement masks and unordered-list rules for the persisted structured
result and observation. It validates generated invocation UUIDs and reservation
identities, preserves control reservations as distinct values, and compares
the retained row contents and counts. D8 provisioning and D11/D12 rejection
differences stay within their existing owner decisions. Every raw iteration is
retained, and the verifier independently recomputes the full scenario matrix.

A separate `make parity-m4` process ran concurrently in this checkout, refreshing
the stale M1-M4 evidence (`go-vs-go`, `go-vs-rust`, canaries, oracles, contracts)
under its own output directory; M5 uses a separate one. Its first attempt hit a
real (if minor) intermittent failure: `authz.sqlite`'s `clients.created_at` is
stored as a Unix-epoch integer, and two concurrently-started processes can
round to different wall-clock seconds, which 3/20 `go-vs-go` repeats caught
unmasked. Fixed with an `"int"`-typed mask on that field (not a redaction or
behavior change); reran clean at 6/6 affected scenarios × 20 repeats, then the
full M1-M4 chain passed (`gate m4: PASS`).

A disk-backed differential regression completed at 84/84 scenarios passing
for five repetitions. The harness changed during that run, so its provenance
is marked `changedDuringRun: true`; the verifier rejects it as final evidence.
The preceding run hit a full `/tmp` tmpfs on both implementations and is also
not accepted as product evidence. Validation now puts its own temporary files
under `.parity/m5-tmp` on the workspace filesystem.

The inherited C12/C14 canary waivers still need owner review. This work does
not authorize production cutover. Plan execution, live provider lifecycle,
operator packaging and cutover rehearsal remain with their later milestones;
they must not be inferred from storage tests.

Detailed implementation history: [2026-10-04-progress.md](2026-10-04-progress.md).
Earlier partial claims and their limitations are preserved in
[2026-10-04-early-evidence.md](2026-10-04-early-evidence.md) as historical notes.
