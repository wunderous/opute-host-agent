# m23: no journey takes a tool's word for its own effect

m22 made every step assert its outcome. For a mutation, though, that outcome
is still the tool's own report: a tool that answers success without doing
the work passes. m23 makes the harness require an independent observation.

## The rule

A step is a mutation when its tool's `_meta.capability.effect`, as the run's
own `tools/list` publishes it, is `mutation`, `destructive` or
`credential_bearing`. A task's effect is judged at its first poll; a re-read
of the same task is not a mutation. When a journey expects a mutation step to
succeed, a later step must name it in `confirms` and check the effect with
its own assertions. That later step is a guest command, an HTTP request, a
`fetch`, or a call of a tool whose effect is `read`. Otherwise
`evidence/e2e/run.py` fails the journey with `unconfirmed-effect`. A step that
expects a failure, or a task status other than `completed`, needs no
confirmation. The only opt-out is `effectUnobservable` with its reason. The
lint refuses an empty reason and a `confirms` that names no earlier step.

Offline tests: `Effects` in `evidence/e2e/tests/test_harness.py`. The two
e2e-0 controls are:

| Journey | Expected |
|---|---|
| `selfcheck.unconfirmed-effect`: a stub mutation answers success; nothing confirms it | fail: `unconfirmed-effect` |
| `selfcheck.empty-confirmation`: the same stub; a guest step confirms it and finds nothing | fail: `pass-condition` |

## Inventory

`evidence/e2e/outcomes.py` lists the unconfirmed steps, using the effects
recorded in the gates' committed runs. `make e2e-test` runs it with `--check`.

| Branch | Journeys | Mutating tools exercised | Unconfirmed steps before | After | File |
|---|---|---|---|---|---|
| `main` (`fbe5970fa`) | 41 | 59 | 217 | 0 | [`inventory-main-before.json`](inventory-main-before.json) |
| `rust-0.4.0` (`a12d96818`) | 45 | 68 | 279 | 0 | [`inventory-0.4.0-before.json`](inventory-0.4.0-before.json) |

Each step is now confirmed in the journey that owns it. The confirmation
reads back what the call changed: a file's content or digest, a unit's
`MainPID`, `ActiveState` or linger, a guest's state or address, a cluster
object or its absence, a database file, a listening port, the fetched page.
Guest reads come first; a `read` tool is used where the guest cannot see the
effect. On main, 127 observer steps confirm the mutations. 26 steps opt out,
each with its reason:
- a command that only prints or sleeps, whose output or completion is asserted;
- a read through a general command tool: `kubectl rollout status`, `get`,
  `logs`, `exec` of `echo`, `curl`, `nproc`;
- an `ensure-ha-endpoint` probe;
- a dry run;
- a repeat of an applied removal;
- a write the quota refuses, which its own command removes.

`rust-0.4.0` adds three more opt-outs: console input, console resize, and a
read of the guest's network interfaces.

## Measure

Before any draft ran, the new assertions were replayed against the committed
receipts (`m22-r1`). Every assertion on an existing step held. The only
failures were new observer steps with no record yet, and two existing
guest steps whose output the round extended (`limitsV`, `incusActive`). The changed
journeys then ran once on `rust-0.3.0-rc7`, as the drafts `m23-d1` and `m23-d2`.
Drafts are not committed.

| Id | Journeys | Class | Finding | State |
|---|---|---|---|---|
| J2 | `e2e2.data` (both lines) | journey defect | `sqliteGone` counted files under `~/.opute/databases`, a path the agent never uses. The agent keeps caller databases under `~/.config/opute/instances/standalone/databases` (`config.rs`, `sqlite_root`), so the removal check could never fail, and cleanup left the databases behind | fixed: one `SQLITE` root for the check, the new `sqliteFile` and the plans' read-backs, and for every cleanup (`e2e4` keeps Go's old path too) |
| J3 | `e2e1.restart` | journey defect, in m23's own draft | The new `resubmitDb` read-back assumed the resubmission left the run `unknown`. The resubmission resumes the same durable run: one run id, now `running` (`m23-d1`). Q15 requires only that the resubmission is admitted | fixed: the read-back asserts the same run id and a status of `running` or `completed`; passes in `m23-d2` |
| N1 | `e2e2.oci` | observation | `configure_oci_storage` sets 3600 s, which is also the default, so `inspect_container_storage` shows the same value before the call | the persisted `oci-storage-policy.json` is read instead |

No product defect was found. There is no D53 and no new candidate: every
confirmed effect was present on rc7.

## Candidates

Each line kept its candidate and reran every gate under both rules, under
one new run id:
- `rust-0.3.0-rc7` (main): `m23-r1`. Every journey passed on its first
  attempt, and `evidence/e2e/verify.py --gate` passes every gate. The two
  controls fail as expected: `selfcheck.unconfirmed-effect` with
  `unconfirmed-effect` alone, and `selfcheck.empty-confirmation` with
  `pass-condition` alone.
- `rust-0.4.0-rc4` (`rust-0.4.0`): `m23-r2`. The journeys with steps only
  0.4.0 runs drafted first as `m23-d3` (not committed); every gate then
  passed on its first attempt, and `verify.py --gate` passes each. e2e-7
  created and removed its own `e2e-m23-r2.opute.io` record, as the m20
  decision allows.
