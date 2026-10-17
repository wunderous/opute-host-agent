# M8b: incus domain (evidence)

M8b ports the Go `internal/domain/incus` mutating operations to Rust. They are
built-in providers of four three-role services (D14):

- **instance-lifecycle:** `create_vm`, `provision_vm`, `start_vm`, `stop_vm`,
  `restart_vm`, `update_vm_resources`, `delete_vm`
- **system-containers:** `provision_container`, `probe_gpu_container`
- **virtualization-stack:** gains `reset_virtualization_stack` (D21 alias
  `reset_incus_stack`)
- **local-status:** `check_local_prerequisites`, `get_local_status`

Parity with the pinned Go baseline is proven at two tiers, as in M8a:

- **T1:** recording shims.
- **T2:** disposable twin VMs, here with a nested Incus inside each twin.

The Go authority is `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`), with
reference binary `71924b62…`. The Rust candidate is `e5468fbb…`, built with
`cargo build --release --locked` from round head `a80c433c`.

**Result:** the m8b gate passes, and gates m0–m6 and m8a still pass. The
cutover gate fails as expected, on 28 owned inventory gaps and four cutover
evidence files that belong to later milestones.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (every command) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| T1 Go/Go ×20 and Go/Rust ×5, all 276 manifest items | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 Go/Go ×20 and Go/Rust ×5, 19 scenarios (14 host, 5 incus) | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), [golden](t2-golden.json), [nested-Incus golden](t2-golden-incus.json) |
| Go `test/live` oracle inside the twins | [oracles-t2.json](oracles-t2.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [cutover](verify-cutover.json) |
| Contracts, canaries and client oracles | [contracts/](contracts/), [Go canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [go-sdk OAuth client](oauth-sdk-client/receipt.json) |
| M5 durable state, including the 16-field secret sweep | [m5/](m5/) |
| D14 seam stage | [report](../seam/m8b/report.json) |
| Inventory | [effects](../../baseline/inventory/effects.json), [t2](../../baseline/inventory/t2.json), [gaps](../../baseline/inventory/gaps.json) |

## What passed in round 3

All 31 steps exited 0. The tree was clean before and after, and the binaries
did not change during the round. Nothing else heavy ran on the host.

| Check | Result |
| --- | --- |
| fmt, Clippy `-D warnings`, workspace tests, locked release build | pass; 239 tests |
| `make spec-validate`, catalog source, harness tests | pass; 15 specs |
| Owning Go tests: incus and host domains, `hostruntime` and `resource` ×5; core packages | pass; 820 owning passes |
| T1 Go/Go ×20 | 276/276 |
| T1 Go/Rust ×5 | 276/276 |
| T2 Go/Go ×20 | 19/19 |
| T2 Go/Rust ×5 | 19/19 |
| Go `test/live` oracle in a twin | Rust's per-test outcome equals Go's; negative control passes nothing |
| Go canaries / Rust canaries | 14/14 caught / 24/24 caught |
| Contracts D8, D10, D11, D12, D13 | 24/24, 8/8, 3/3, 3/3, 4/4 |
| M5 crash, shape, migration, cross-read, unknown projection | 200/200, 1365/1365, 6/6, 18/18, 0 failures |
| Secret sweep | 16 marked fields, 0 failures |
| Seam verifier | 68 compliant, 82 unported, **0 gaps**; dispatch canary red |

## T2 sandbox

The twins are the M8a twins (see [M8a](../m8a/README.md#t2-sandbox)), booted
from a second golden, `golden-incus` (`25356ecc…`, 6 GiB). It is an overlay of
the base golden with Incus from the Zabbly stable repository inside the twin:
a 24 GiB `btrfs` pool, the managed bridge `incusbr0`, and cached container and
VM images (`parity-noble`, `parity-noble-vm`). Snapshots add an Incus
projection: instances with their config and devices, storage volumes,
networks, profiles and images. The projection drops `volatile.*` keys (MACs,
UUIDs, the last state) and runtime state.

The five incus scenarios are:

- `vm.lifecycle`: create, provision, start, stop, restart, resize and delete.
- `container.lifecycle`: provision with a static IP, probe the GPU, and
  delete.
- `reset.owned-container`: the reset guards, the checkpoint, the owned
  deletes and the reinstall.
- `standalone.reads`: the local prerequisites and status.
- `admission.rejected-no-effect`: reruns every incus call that M4 refuses.
  Its delta stays empty on both twins.

`exec_command` and `run_instance_command`, left to M8b by M8a, now run
against a real nested instance.

The Incus is nested inside the disposable twin. No shared Incus, Kubernetes,
tunnel or deployment state was touched.

### Go `test/live` oracle

The milestone names three Go tests:

- `TestLiveVMCreateListDelete`
- `TestLiveGetHostInfoIncus`
- `TestLiveResetIncusStackDeleteReconcileAndVerify`

They run unchanged against a binary inside a twin. At the pinned commit, two
of them do not pass against the Go reference itself. The go-sdk v1.7.0 client
declares no tasks extension, so the 2026-07-28 server refuses task-aware
`create_vm`, and the reset test skips on its fixture.

The oracle therefore holds Rust to Go's own outcome:

- `TestLiveGetHostInfoIncus` must pass on both binaries.
- Rust's per-test outcome and failure messages must equal Go's.
- A negative control must pass nothing. It did: all three failed.

## Normalizations and divergences

Every normalization is declared in a scenario or in
[divergences.json](../../tools/parity/divergences.json).

**D21** (owner decision, `openspec/changes/neutral-instance-tool-names`)
extends D19's one name map:

- `reset_incus_stack` is published as `reset_virtualization_stack`.
- `provision_container.wslGpuLibs` is published as `hostGpuLibs`.

Go's names stay hidden aliases. A tool that keeps its Go name still accepts
Go's field.

The existing `D19.neutral-host-tool-names` rule rewrites Rust's side back to
Go's spelling. Every Rust catalog revision was re-pinned with its Go pair:
both served catalogs and the 14 provider-fixture pairs. Each pair was
observed one-to-one.

**T2 masks** come from the scenario file: the DHCP lease on `incusbr0`, live
host load and memory figures (their shape is compared in T1), and wall-clock
timestamps.

## Harness changes

Each harness change is committed separately:

- a `readFile` step, so a durable file such as the reset checkpoint is
  compared, not only its existence;
- shim `removes`, so a deleted instance drops out of later listings;
- sandbox cleanup after capture;
- the nested-Incus golden and the Incus projection;
- running Go live tests against a binary inside a twin;
- judging that oracle against Go's own outcome.

## Rounds

Only round 3 counts. Earlier rounds were retired. Their failures were
recorded, never relabelled, and their raw logs are kept under
`.parity/retired/`.

| Round | Outcome | Cause |
| --- | --- | --- |
| 1 | retired | host reboot during `t2-go-vs-go`; every completed step had exited 0 |
| 2 | retired | `m5-shape` 1360/1365. See below. |
| 3 | **pass** | |

In round 2, all five `m5-shape` failures were `incus-domain.provision.existing`.
Go's 8-CPU, 16 GiB `provision_vm` was refused with `host_capacity_saturated`,
and Rust's was not. The cause was load on the host: an M8c inventory capture
and a 6 GB k3s golden build were running at the same time, so host admission
saw less free memory on one side.

Rerun alone, the scenario was 5/5 identical. Round 3 ran with nothing else
heavy on the host.

## Open, owned gaps

These come from [gaps.json](../../baseline/inventory/gaps.json): 28 gaps, 0
unowned. Every incus-domain tool is covered at T2 except `exec_command`,
which is inert there and T1 proven.

| Owner | Gaps |
| --- | --- |
| M8a | 3: `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution`, which need a WSL2 runner |
| M8c | 1: `helm` |
| M8d | 2 |
| M8f | 2 |
| M10 | 1 |
| M8, environment reads for later domains | 19 |
