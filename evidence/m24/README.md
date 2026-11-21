# m24: no journey allows a host change it does not make

m23 confirmed each expected effect. Nothing bounded what else a journey's
`effects` let happen: a pattern that matches nothing, or one broader than
the change it covers, would hide an unintended change. m24 makes every
pattern name a change of the journey's own run.

## The rule

`evidence/e2e/run.py` fails a journey with `unused-allowance` when an
`allow` pattern matches no path its run changed, or a `persist` pattern
matches no residue after cleanup. It fails with `broad-allowance` when a
pattern's literal part, before its first wildcard, ends at a `/` or names
less than an object: under `files//`, the first two path components;
elsewhere, a kind, its group and an object (`system/units/<unit>`). So
`files//etc*`, `files//etc/systemd/system/*`, `incus/*` and
`incus/volumes/default/image/*` are broad; `files//opt/opute-e2e/providers*`
and `system/units/e2e-provider-k3s*` are not. A pattern whose match depends
on the host goes in `effects.optional`, mapping the `allow` pattern to its
reason; it is exempt from both checks, and the lint refuses an empty reason.

Offline tests: `Allowances` in `evidence/e2e/tests/test_harness.py`. The two
e2e-0 controls are:

| Journey | Expected |
|---|---|
| `selfcheck.unused-allowance`: writes one file, allows it and a file it never writes | fail: `unused-allowance` |
| `selfcheck.broad-allowance`: writes one file and allows `files//etc*` | fail: `broad-allowance` |

## Inventory

`evidence/e2e/outcomes.py` lists unused and broad patterns per journey from
the states in the gates' committed runs. `make e2e-test` runs it with
`--check`. The two new controls are excluded from the before counts.

| Branch | Journeys | Allow / persist patterns | Unused allow | Unused persist | Broad | File |
|---|---|---|---|---|---|---|
| `main` (`64cf5d34e`) | 43 | 331 / 24 | 122 | 14 | 46 | [`inventory-main-before.json`](inventory-main-before.json) |
| `rust-0.4.0` (`b7408ff6e`) | 47 | 386 / 27 | 138 | 17 | 61 | [`inventory-0.4.0-before.json`](inventory-0.4.0-before.json) |

Both inventories apply the rule above to the journeys as they were before
m24 (`m24-inv` replays them with m24's checks).

## Narrowing

Each pattern now names what the run changes: the instance, its volume, the
image volume by its fingerprint, the unit, the package's dpkg entries, the
file. Shared lists live in the generators:
- `make_e2e3a`: `IMAGE_VOLUME` (the snapshot's `parity-noble` image), the
  three-server lists;
- `make_e2e4a`: `USER_SESSION` (linger and the opute user manager's units),
  `STACK_REINSTALL` (what `reset_virtualization_stack` reinstalls),
  `guest_effects(...)`, the podman packages and binaries, `SLOW_UNIT`;
- `make_e2e6`: `units(...)` and each journey's agents by name;
  `SWEEP_EFFECTS` (the cluster agent install and the connector's tunnel).

No pattern is optional on main.

## Findings

| Id | Journeys | Class | Finding | State |
|---|---|---|---|---|
| W1 | `e2e2.host-ops` | journey defect | `allow` named `files//home/opute/evil`, the file a path-traversal escape would write. Had the guard failed, the escape would have passed the effects check | removed |
| W2 | `e2e2.host-ops`, `e2e2.oci`, `e2e3.site` | journey defect | 8 patterns named `packages/...`, `users/...` and `groups/*`, kinds the state never records, so they could match nothing | removed |
| W3 | e2e1, e2e2, e2e3, e2e4, e2e6 | journey defect | Units that steps stop before the after-state were allowed: the slow-endpoint helper outside `e2e6.kill`, the release server and the user manager's units once `e2e1.user-unit-npm` stops them, workload units, `systemd-timedated`, `run-user-1000.mount` (opute is uid 1001), and tap device units no run left. Each matched nothing | removed |
| W4 | 17 journeys (e2e1, e2e2, e2e3, e2e3a, e2e4, e2e6) | journey defect | Broad patterns (`files//usr*`, `files//etc*`, `files//var*`, `system/*`, `incus/*`, `user/*`, `files//etc/systemd/system/*`, `files//etc/opute/*`, `files//opt/opute/*`) allowed any change in a tree | replaced by the paths each run changes |
| W5 | 8 journeys (e2e2, e2e3, e2e4) | journey defect | `persist` named residue the run never left: `~/.opute*` (5 journeys), and `/var/lib/apt*`, `/var/lib/dpkg*`, `/var/cache*`, `/var/log*` (`e2e2.incus`, `e2e2.oci`, `e2e3.site`) | removed |
| W6 | `e2e2.incus` | observation | `reset_virtualization_stack` reinstalls the stack (`install_incus_stack`): `/opt/incus`, the incus unit and dpkg entries change, and stay | intended; allowed and persisted as `STACK_REINSTALL` |

No product defect was found.

## Measure

The narrowed patterns were first replayed against the committed states
(`m23-r1`, `m23-r2`): no unused, broad or unjustified pattern was left on
either branch. Every changed journey then ran once on `rust-0.3.0-rc7`, as
the draft `m24-d1` (30 journeys; drafts are not committed). Every journey
passed, and the two controls failed with their own failure alone. The draft
found nothing the replay had not: no change outside the narrowed patterns and
no pattern its live run left unused. On `rust-0.4.0`, its 34 changed journeys
ran once on `rust-0.4.0-rc4` as the draft `m24-d2` (VM built from that
branch's `devvm.py`), with the same result.

## Candidates

Each line kept its candidate and reran every gate under the three rules,
under one new run id:
- `rust-0.3.0-rc7` (main): `m24-r1`. Every journey passed on its first
  attempt, and `evidence/e2e/verify.py --gate` passes every gate. The new
  controls fail as expected: `selfcheck.unused-allowance` with
  `unused-allowance` alone, and `selfcheck.broad-allowance` with
  `broad-allowance` alone.
- `rust-0.4.0-rc4` (`rust-0.4.0`): `m24-r2`. Every journey passed on its
  first attempt, and `verify.py --gate` passes every gate.
