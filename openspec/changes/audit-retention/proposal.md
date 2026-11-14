# Audit retention

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D42.** Goal m17.

## Why

Every tool call writes one row to `capability_invocations`, the durable
audit, and one to `operations`, which also holds a task's snapshot. Nothing
ever prunes either (m16 observation O1). Under
the e2e-6 budget load, about 3.5 KB per call went to the store, 81.5 MB in
15 minutes, and the state directory grows until D31's full-disk refusal
stops every effectful call. The spec states no retention.

The failing-first test
`store::tests::audit_rows_are_pruned_by_age_and_count_but_never_while_referenced`
kept every row on rust-0.4.0-rc1's behaviour.

## What Changes

- Audit rows are kept for `OPUTE_AUDIT_RETENTION_DAYS` days (default 30)
  and at most `OPUTE_AUDIT_RETENTION_ROWS` rows (default 50000), the
  newest first. A value that is not a positive integer means the default,
  as `OPUTE_OAUTH_CONSENT_DAYS` does.
- The agent prunes at start and then every 60 seconds, off the request
  path. A row whose operation is still `working` is never pruned, whatever
  its age or position.
- A finished operation (`completed`, `failed` or `cancelled`) is pruned
  in the same pass once two things hold. It is older than the age, or older
  than every audit row kept, and no kept audit row names it. And its task's
  one-hour TTL has passed since its last update, so `tasks/get` and
  `get_operation` answer for at least that long, as before. Kept whatever
  their age: an operation still `working`; one `unknown` after a restart,
  which can still be cancelled; and one a plan run or an active capability
  names by its run id.
- Plan runs, active capabilities and resource records are not pruned.
  Neither agent reads an audit row back: Go v0.2.4 only creates and
  inserts them (`internal/state/store.go`). A Go rollback reads the
  operations that remain.
- SQLite reuses the pages pruned rows free, so the state file stops
  growing at the bound rather than shrinking.

Measured on a draft of this change (`e2e6.budget`, 15 minutes, 27,344
calls, a 5,000-row bound): the audit ended at exactly 5,000 rows, and the
store grew about 0.65 MB a minute over the last five minutes. Before this
change it grew about 5.4 MB a minute (81.5 MB in 15 minutes, O1). The rest is operations still
inside their task's TTL, which the next pass after the hour prunes.

The defaults: 50,000 rows is about 175 MB at the measured 3.5 KB per call,
and more than a month of one call a minute. 30 days covers the 14-day rollback
window (D28) with room to inspect it.

## Options rejected

- **Age only.** A burst like the budget load would still fill the disk
  within the age window.
- **Rows only.** An idle host would keep year-old rows nobody needs.
- **Leaving operations unpruned.** Every call writes one, about 350 bytes,
  so the store would keep growing with the call count.
- **Pruning operations as soon as their audit rows go.** A finished task
  would vanish from `tasks/get` within its TTL under heavy load.
- **A size bound in bytes.** Measuring it needs a scan of every row's
  text each time. A row count bounds the store as well at the measured
  per-row size, and the budget journey checks the bytes.
- **VACUUM after pruning.** It rewrites the whole file under the
  writer's lock and needs as much free space again. Page reuse bounds
  the file without it.

## Impact

`store.rs` (`prune_audit`, `AuditRetention`), `config.rs` (the two
settings), `app.rs` (the background pruner), the failing-first test, and
the `e2e6.budget` journey, which now shows the store bounded. A product
change: the next candidate is rust-0.4.0-rc2.
