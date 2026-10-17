# M8c: kubernetes and cluster domains (evidence)

M8c ports the Go `internal/domain/kubernetes` and `internal/domain/cluster`
operations to Rust as built-in providers of three-role services (D14):

- **cluster resources:** `apply_manifest`, `get_cluster_resource`,
  `get_cluster_resource_status`, `delete_cluster_resource`,
  `list_cluster_events`, `put_cluster_secret`, `exec_cluster_command`
- **cluster packages:** `install_cluster_package`, `render_cluster_package`
- **exposure:** `configure_service_domain`, `remove_service_domain`
- **guest storage:** `inspect_guest_storage`, `trim_guest_storage`,
  `prune_unused_cluster_images`, `garbage_collect_cluster_registry`
- **cluster agent:** `install_cluster_agent` and the bridge relay
- **discovery and listings:** `list_compute_clusters`, `list_namespaces`,
  `list_storage_classes`, `list_ingress_classes`, `list_certificate_issuers`,
  `list_pods`, `list_services`, `list_deployments`. Go's unreachable `list_clusters`,
  `get_cluster_details`, `get_cluster_runtime_details` and
  `uninstall_helm_chart` are refused exactly as Go refuses them.

`configure_agent_connection` was ported in M8a and has no cluster part; it is
not ported again.

Every kubernetes call goes through the provider capability
`opute.capability.kubernetes.v1`, served by the pinned Go k3s provider.

Parity is proven at two tiers:

- **T1:** recording shims, 11 cluster-domain scenarios against the real
  pinned provider.
- **T2:** disposable twin VMs with a real single-node k3s, a registry and
  helm.

The Go authority is `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`), with
reference binary `71924b62…`. The Rust candidate is `9c3c23ac…`, built with
`cargo build --release --locked` from round head `28d79438`.

**Result:** the m8c gate passes, and gates m0–m6, m8a and m8b still pass. The
cutover gate fails as expected, on 21 owned inventory gaps and four cutover
evidence files that belong to later milestones.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (every command) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| T1 Go/Go ×20 and Go/Rust ×5, all 287 manifest items | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 Go/Go ×20 and Go/Rust ×5, 24 scenarios (14 host, 5 incus, 5 cluster) | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), goldens: [base](t2-golden.json), [incus](t2-golden-incus.json), [k3s](t2-golden-k3s.json) |
| Go `test/live` oracle inside the twins | [oracles-t2.json](oracles-t2.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [cutover](verify-cutover.json) |
| Contracts, canaries and client oracles | [contracts/](contracts/), [Go canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [go-sdk OAuth client](oauth-sdk-client/receipt.json) |
| M5 durable state, including the 16-field secret sweep | [m5/](m5/) |
| D14 seam stage | [report](../seam/m8c/report.json) |
| Entry and naming record | [NOTE.md](NOTE.md) |
| Inventory | [effects](../../baseline/inventory/effects.json), [t2](../../baseline/inventory/t2.json), [gaps](../../baseline/inventory/gaps.json) |

## What passed in round 4

All 32 steps exited 0. The tree was clean before and after, and the binaries
did not change during the round. Nothing else heavy ran on the host.

| Check | Result |
| --- | --- |
| fmt, Clippy `-D warnings`, workspace tests, locked release build | pass; 245 tests |
| `make spec-validate`, catalog source, harness tests | pass; 16 specs |
| Owning Go tests: kubernetes, cluster, `hostmcp`, `tcprelay` and the k3s provider ×5; core packages | pass; 875 owning passes |
| T1 Go/Go ×20 | 287/287 |
| T1 Go/Rust ×5 | 287/287 |
| T2 Go/Go ×20 | 24/24 |
| T2 Go/Rust ×5 | 24/24 |
| Go `test/live` oracle in a twin | Rust's per-test outcome equals Go's; negative control passes nothing |
| Go canaries / Rust canaries | 14/14 caught / 24/24 caught |
| Contracts D8, D10, D11, D12, D13 | 24/24, 8/8, 3/3, 3/3, 4/4 |
| M5 crash, shape, migration, cross-read, unknown projection | 200/200, 1420/1420, 6/6, 18/18, 0 failures |
| Secret sweep | 16 marked fields, including `put_k8s_secret.data` and `install_cluster_agent.bridgeToken` in both modes; 0 failures |
| Seam verifier | 92 compliant, 58 unported, **0 gaps**; dispatch canary red |

## T1

[`tools/parity/cluster_domain_scenarios.py`](../../tools/parity/cluster_domain_scenarios.py)
generates the matrix. Each scenario installs the pinned Go k3s provider
exactly as `provider-real.k3s` does. The provider's `incus exec k1 -- k3s
kubectl …` calls are answered by the recording shim, so the trace holds the
provider argv and stdin hash on both sides.

- **The scenarios:** discovery, resources, listings, packages, exposure,
  storage, agent, three agent variants and the unreachable tools.
- **The agent variants:** `OPUTE_K3S_VM=c1`, `OPUTE_DOGFOOD_GATEWAY_VM=k1`,
  and `OPUTE_K3S_CONTAINER=absent`, each with `OPUTE_PLATFORM_GUEST_HOST`.
  Together with the platform URLs, they cover the six cluster environment
  reads.
- **Calls** use Go's names. The D19 divergence maps Rust's published names
  and catalog revisions back.
- **After the install,** each scenario waits until `get_host_capacity`
  reports no reservation and no heavy operation in flight. The install task
  reports completed before its reservation is released, and Go refuses a
  normal call in that window.

## T2 sandbox

The k3s golden (`481c5443…`) is an overlay of the nested-Incus golden
(`25356ecc…`):

- **k1:** a privileged Incus container running a single-node k3s
  `v1.31.2+k3s1` from the release's airgap images. Traefik, servicelb,
  metrics-server and the helm controller are disabled.
- **Registry:** a Deployment `registry/opute-registry` (`registry:2.8.3`,
  delete enabled) with a local-path PVC and its config, seeded with two tags.
- **helm:** `v3.16.2` on the twin.
- **Pinning:** every download is pinned by sha256.

The provider runs as `ubuntu` in a transient unit and reaches k1 through
`incus exec`, as on a real host.

Snapshots add a Kubernetes projection:

- objects from `kubectl get -o json`, without volatile metadata and status,
  with Secret data as sha256
- the registry catalog and tags
- k1's images
- k1's files under `/etc/opute`, `/usr/local/lib/opute` and
  `/etc/systemd/system`, and its units

The five cluster scenarios:

- `resources.lifecycle`: apply, read, status, events, a secret, exec,
  delete, and the exposure refusals. Teardown is empty.
- `packages.chart-and-render`: a HelmChart installed and deleted through the
  provider, and `helm template` on the twin. The residue is Go's chart files,
  reproduced.
- `storage.prune-and-collect`: inspect, prune, trim, and registry garbage
  collection. GC deletes two tags and restores the registry.
- `agent.vm-install`: `install_cluster_agent` against a harness-owned platform
  stub inside the twin (127.0.0.1:9093). The agent files and an active unit
  in k1 are Go's residue, reproduced.
- `admission.rejected-no-effect`: reruns every cluster call that M4 refuses.
  Its delta stays empty on both twins.

`configure_service_domain`, `remove_service_domain` and `uninstall_helm_chart`
are inert at T2. Go takes the VM only from a binding that no catalog argument
can supply, so they always refuse.

No shared Incus, Kubernetes, tunnel or deployment state was touched, and no
real platform was contacted.

## Normalizations and divergences

Every normalization is declared in a scenario or in
[divergences.json](../../tools/parity/divergences.json).

- **D22** (owner decision, `openspec/changes/neutral-cluster-tool-names`)
  extends the one D19 name map with nine renames: `*_k8s_*` → `*_cluster_*`,
  `exec_kubernetes_command` → `exec_cluster_command`,
  `list_kubernetes_clusters` → `list_compute_clusters`, and `*_helm_*` →
  `*_cluster_package`. Go's names stay hidden aliases.
- **D19's rule** maps Rust's side back to Go's spelling, as before. The 14
  provider-fixture revision pairs were re-pinned, each observed one-to-one.
- **Canonical comparison:** a dict list keeps its order when re-sorted after
  the rename. The canonicalizer now re-sorts by every sort key present when
  the original is sorted that way.
- **T2 masks** come from the scenario file:
  - server-assigned object metadata
  - the agent's task ids
  - storage byte counts
  - the incus client's first-run tip
  - each twin's memory figures

## Defects fixed while proving parity

- **Rust:** a plan node that names a D19 alias (for example `put_k8s_secret`)
  is now validated and redacted through Go's own descriptor. Before, Rust
  refused such a plan and stored its arguments as `{"redacted": true}`. Go
  redacts only the write-only field. The round 3 secret sweep found it.
- **Rust:** a Clippy `useless_conversion` in a test.
- **Harness:** each fix is committed separately.
  - the post-install reservation wait
  - the secret sweep deriving Rust's coverage in Go's spelling
  - the incus `provision.existing` request size (below)
  - the cluster T2 setup listing names and states only
  - the seam canary tree including the cluster formatting oracle

## Rounds

Only round 4 counts. Rounds 1–3 were stopped or retired. Their failures were
recorded, never relabelled, and their raw logs are kept under
`.parity/retired/m8c-round*/`.

| Round | Outcome | Cause |
| --- | --- | --- |
| 1 | stopped | Clippy `useless_conversion` in a `clusterops` test |
| 2 | stopped | T1 Go/Go 286/287. See below. |
| 3 | retired | four steps failed. See below. |
| 4 | **pass** | |

**Round 2:** `cluster-domain.exposure` failed 1 in 20. Go refused its first
call `host_capacity_saturated` because the provider install's reservation
had not yet been released.

**Round 3** failed four steps, each with a separate cause:

- **Secret sweep:** D22 alias coverage, and the Rust redaction defect above.
- **M5 shape, 1416/1420:** the cause is below.
- **T2 Go/Go 23/24:** k1's address list in a setup step's output changed
  between twins.
- **Seam:** the canary did not compile, so it could not turn red.

### M5 shape and the M8b record

M5 shape failed on `incus-domain.provision.existing`. Admission computes
effective memory as the host's total less the larger of meminfo usage and
the agent cgroup's `memory.current`. That cgroup includes about 13 GiB of
page cache here, so a 16 GiB heavy request sits at the limit, and either
side can be refused.

This is also the real cause of M8b round 2. That round was recorded as
concurrent load, but the extra load only grew the page cache. The scenario
now asks for 4 CPUs and 4 GiB, which is still above `vm1`'s 2 CPUs and 2 GiB.

## Open, owned gaps

These come from [gaps.json](../../baseline/inventory/gaps.json): 21 gaps, 0
unowned. No M8c gap remains:

- The `helm` effect tier is `T2-sandbox`.
- The six cluster environment reads are covered.

| Owner | Gaps |
| --- | --- |
| M8a | 3: `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution`, which need a WSL2 runner |
| M8d | 2: `buildah`, `podman` |
| M8f | 2 |
| M10 | 1 |
| M8, environment reads for later domains | 13 |
