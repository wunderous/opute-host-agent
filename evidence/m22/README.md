# m22: every journey step asserts its outcome

U1 (D52) hid for two goals: `e2e6.secret-sweep` accepted a failed connector
call, and m21's first draft asserted only the task status. m22 makes the
harness refuse that.

## The rule

`evidence/e2e/run.py` fails a journey with `unchecked-outcome` when a
`tools/call` or `tasks/get` step:
- has no assertion deciding its outcome. A deciding assertion is on
  `isError`, on the result's content or `structuredContent`, on a task's
  own result, on a JSON-RPC error, or on a task status other than
  `completed`. A task launch is judged at the later call or poll that uses
  its captured id; or
- returns `isError: true`, a task result with `isError: true`, a failed task
  or a JSON-RPC error, and no assertion of that step accepts the failure.

A step may opt out only with `expectAnyOutcome` and its reason. The offline
tests are `Outcomes` in `evidence/e2e/tests/test_harness.py`; the e2e-0
control is `selfcheck.unchecked-outcome`, a stub answering `isError: true`
whose text alone is asserted, which must fail with `unchecked-outcome`.

## Inventory

`evidence/e2e/outcomes.py` lists the flagged steps; `make e2e-test` runs it
with `--check`. Before this round, on each branch's committed journeys:

| Branch | Journeys | `tools/call` and `tasks/get` steps | Flagged | File |
|---|---|---|---|---|
| `main` (`916d5567f`) | 40 | 612 | 48 | [`inventory-main-before.json`](inventory-main-before.json) |
| `rust-0.4.0` (`afef7e3f2`) | 44 | 746 | 78 | [`inventory-0.4.0-before.json`](inventory-0.4.0-before.json) |

The goal's count, 269 of 746 steps without an assertion on `rust-0.4.0`,
includes the task launches, which the rule judges at their polls. After this
round both branches report zero.

## Measure

Each flagged step was first replayed against the committed receipts of the
current candidates: `rust-0.3.0-rc7` (`m21-r1`) and `rust-0.4.0-rc4` (its
gates' runs). With the new assertions every receipt re-derives its verdict
except `e2e6.store-damage`, and two journeys whose new task polls have no
record yet. Those three ran once on rc7 as the draft `m22-draft1`, which is
not committed.

| Id | Journeys | Class | Finding | State |
|---|---|---|---|---|
| J1 | `e2e6.store-damage` (both lines) | journey defect | The agent ran as `opute`, which has no user manager on the VM, so its one task failed with `Failed to connect to bus`; the journey asserted only the status `completed` | fixed: the agent runs as root, as the other e2e-6 agents with tasks do, and the task's `kept` output is asserted; passes in `m22-draft1` |
| E1 | `e2e1.restart` `killTask` | expected refusal | A plain `run_host_plan`'s in-memory task is never persisted, so after the SIGKILL `tasks/get` for it answers the JSON-RPC error `task not found: …` (Q15, D28) | asserted |
| E2 | `e2e4.cutover` `goSupervisor` | expected (Go, frozen) | Go v0.2.4's first `ensure_host_service_supervisor` enables linger and may answer before the user bus is up: `user service supervisor bus is unavailable` | `expectAnyOutcome` with that reason; `goSupervisorAgain` asserts `ready` |
| E3 | `e2e6.secret-sweep` `tunnel`; on `rust-0.4.0` also `e2e4.upgrade-chain`'s six tunnel calls and re-reads | expected refusal | The endpoint is `.invalid` by design: each call gets past its origin probe and stops at the public one | `isError: true` and the text `Error: probe authenticated public MCP endpoint: public MCP endpoint did not become ready within …` asserted |
| E4 | `e2e3.discovery` `configmapKeys`, `secretKeys` (main) | expected refusal | 0.3.0 publishes both tools with no provider (Q18) | `isError: true` and the exact `… is not implemented by this Host Agent build` text asserted; 0.4.0 implements both (D32, D33) |

No product defect was found, so there is no D53 and no new candidate. Every
other flagged step returned success on both candidates; each now asserts its
result.

## Candidates

With no product finding, each line kept its candidate and reran every gate
under the rule under one new run id:
- `rust-0.3.0-rc7` (main): `m22-r1`, every gate passes
  (`evidence/e2e/verify.py --gate`). The first attempts of `e2e2.host-ops` and
  `e2e2.oci` failed on downloads (the Ubuntu archive at 20-37 kB/s; the
  package steps' deadlines expired). They were retired with that cause to
  `.parity/retired/m22-r1-apt-slow/` and each reran once under `m22-r1`.
- `rust-0.4.0-rc4` (`rust-0.4.0`): recorded on that branch.

`selfcheck.unchecked-outcome` fails with `unchecked-outcome` alone in
`m22-r1`, as e2e-0 expects.
