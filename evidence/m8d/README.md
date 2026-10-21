# M8d: oci domain (evidence)

M8d ports the Go `internal/domain/oci` operations to Rust as built-in
providers of three-role services (D14):

- **oci registry:** `install_oci_registry`, `get_oci_registry_status`,
  `delete_oci_registry`, with the registry's storage admission
- **oci builder:** `ensure_oci_builder` and the container runtime adapters
- **build context:** `stage_build_context`
- **image build:** `build_and_push_oci_image`
- **container storage:** `configure_oci_storage`,
  `inspect_container_storage`, `cleanup_container_storage`

The registry tools and `configure_oci_storage` reach the cluster through the
provider capability `opute.capability.kubernetes.v1`, served by the pinned
Go k3s provider, as in M8c. The tool names stay Go's. D23 publishes neutral
values for the `builder` and `runtime` selectors ([NOTE.md](NOTE.md)).

Parity is proven at two tiers:

- **T1:** recording shims, 18 oci-domain scenarios.
- **T2:** disposable twin VMs with real rootless podman and buildah, and a
  real registry deployed in k3s.

The Go authority is `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`), with
reference binary `71924b62…`. The Rust candidate is `bc2d5457…`, built with
`cargo build --release --locked` from round head `199774b2`.

**Result:** the m8d gate passes, and gates m0–m6, m8a, m8b and m8c still
pass. The cutover gate fails as expected, on 19 owned inventory gaps and four
cutover evidence files that belong to later milestones.
All 18 oci-domain items and the three OCI T2 scenarios moved from
`unverified` to `pass` in both suites.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (every command) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| T1 Go/Go ×20 and Go/Rust ×5, all 305 manifest items | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 Go/Go ×20 and Go/Rust ×5, 27 scenarios (14 host, 5 incus, 5 cluster, 3 oci) | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), goldens: [base](t2-golden.json), [incus](t2-golden-incus.json), [k3s](t2-golden-k3s.json), [oci](t2-golden-oci.json) |
| Go `test/live` oracle inside the twins | [oracles-t2.json](oracles-t2.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [m8d](verify-m8d.json) [cutover](verify-cutover.json) |
| Contracts, canaries and client oracles | [contracts/](contracts/), [Go canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [go-sdk OAuth client](oauth-sdk-client/receipt.json) |
| M5 durable state, including the 16-field secret sweep | [m5/](m5/) |
| D14 seam stage | [report](../seam/m8d/report.json) |
| Entry and naming record (D23) | [NOTE.md](NOTE.md) |
| Inventory | [oci domain](../../baseline/inventory/oci-domain.json), [effects](../../baseline/inventory/effects.json), [t2](../../baseline/inventory/t2.json), [gaps](../../baseline/inventory/gaps.json) |

## What passed in round 4

All 31 steps exited 0. The tree was clean before and after, and the binaries
did not change during the round. Nothing else heavy ran on the host.

| Check | Result |
| --- | --- |
| fmt, Clippy `-D warnings`, workspace tests, locked release build | pass; 250 tests |
| `make spec-validate`, catalog source, harness tests | pass; 17 specs |
| Owning Go tests: `internal/domain/oci` and `internal/exec` ×5; core packages | pass; 145 owning passes, 0 failures |
| T1 Go/Go ×20 | 305/305 |
| T1 Go/Rust ×5 | 305/305 |
| T2 Go/Go ×20 | 27/27 |
| T2 Go/Rust ×5 | 27/27 |
| Go `test/live` oracle in a twin | Rust's per-test outcome equals Go's; negative control passes nothing |
| Go canaries / Rust canaries | 14/14 caught / 24/24 caught |
| Contracts D8, D10, D11, D12, D13 | 24/24, 8/8, 3/3, 3/3, 4/4 |
| M5 crash, shape, migration, cross-read, unknown projection | 200/200, 1510/1510, 6/6, 18/18, 0 failures |
| Secret sweep | 16 marked fields; 0 failures |
| Seam verifier | 101 compliant, 49 unported, **0 gaps**; dispatch canary red |

## T1

[`tools/parity/oci_domain_scenarios.py`](../../tools/parity/oci_domain_scenarios.py)
generates the matrix. The image tools, `sudo` and `apt-get` are answered by
the recording shim, so the trace holds every argv and stdin hash on both
sides.

- **Builder:** podman present, podman installed but failing, buildkit only,
  a package install, a broken apt and no apt.
- **Stage and build:** a staged context, then builds with podman and buildah,
  and the policy refusals.
- **Storage:** inspect and configure, the policy file, cleanup, a byte
  budget, no runtime, the runtime not installed, and an image another owner
  shares.
- **Registry:** install, status and delete. Go takes the VM from the
  cluster binding's `providerInstanceName`, so every fixture carries the k3s
  estate of M8c and the registry tools bind `cluster:local:k1`.
- **Calls** use Go's names and Go's selector values. The D19 divergence maps
  Rust's published D23 values back. It is declared only where Rust publishes
  one, so it is never stale.
- **Sets:** the staged file list (Go walks a map) is compared as a set, on
  the wire and, by projection, in the durable operations rows.

## T2 sandbox

The OCI golden (`bf332e6b…`) is an overlay of the k3s golden (`481c5443…`):

- **Packages:** podman `4.9.3+ds1-1ubuntu0.2` and buildah
  `1.33.7+ds1-1ubuntu0.24.04.3` from the Ubuntu archive, pinned by deb sha256.
  The timers the packages enable are disabled, as in the base golden.
- **Rootless store:** ubuntu's store holds `localhost/parity/old:1` and
  `localhost/parity/shared` under two tags, built `FROM scratch` with fixed
  timestamps.
- **Session:** the agent runs as a system unit of ubuntu with no user
  session and no `XDG_RUNTIME_DIR`, as on a host where it is installed as a
  service. Podman uses cgroupfs and the file events logger.

Images are built `FROM scratch` out of a context the agent stages, so no image
is pulled from outside the twin. Pushes go only to the registry that
`install_oci_registry` deploys in k1, through its NodePort on k1's reserved
address `10.0.100.113:30500`.

Snapshots add the user's image names (a build's own timestamps make its id
twin-local) and the registry catalog to the cluster projection.

The three OCI scenarios:

- `build.push-to-twin-registry`: both builders, a staged context, and the
  registry installed and waited for. Then three builds: podman pushes
  `app:1` and untags it after the push; buildah's legacy path pushes `app:2`
  and keeps its tag; a TLS push of `app:3` to the plain-HTTP registry is
  refused after its build. The registry is then deleted. The residue is Go's,
  reproduced.
- `storage.policy-and-cleanup`: inspect, the policy file, a dry run, and the
  cleanup that removes the old image and keeps the shared one. A budget then
  holds the policy runs of a build whose push is refused.
- `admission.rejected-no-effect`: reruns every OCI call that M4 refuses,
  aimed at the store, the registry and k1. Its delta stays empty on both
  twins.

No shared Incus, Kubernetes, registry, tunnel or deployment state was
touched, and nothing was pushed to a real registry.

## Normalizations and divergences

Every normalization is declared in a scenario or in
[divergences.json](../../tools/parity/divergences.json).

- **D23** (owner decision, `openspec/changes/neutral-oci-selector-values`)
  extends the one D19 map with a value section: `podman` →
  `container-engine`, `buildah` → `image-builder`, `buildkit` →
  `build-daemon`. Go's values stay hidden aliases. The crate test
  `d23_publishes_neutral_selector_values` shows that a Go value and its
  published value reach the Consumer as the same Go value. D19's rule maps
  Rust's results back to Go's values.
- **T2 masks** come from the scenario file:
  - the registry Deployment's server-assigned metadata and condition times
  - the status answer's text, whose values are server-assigned (the
    Deployment itself is compared in the snapshot)
  - a refused push's layer digest and retry times
- **M5 shape** projects a scenario's sets onto operations rows, as it
  already projected masks.
- **M5 shape, live host rows:** `provider-lifecycle.teardown.idempotent`
  declares its live listing of the developer host's systemd units
  (`liveHostRows`, round 3 below). Each listed row must be a well-formed
  registration of the unit it names, and both sides must have listed or
  neither. Those rows and their count are then left out; every other row is
  compared.

## Defects fixed while proving parity

- **Rust:** `get_oci_registry_status` now answers its JSON as text content
  as well, as Go's `structuredResult(out, "")` does. Before, Rust returned
  structured content only. The T1 registry scenario found it.
- **Harness:** each fix is committed separately.
  - M5 shape projects sets onto operations rows
  - T2 snapshots leave logind session scopes out
  - the OCI golden disables the timers its packages enable
  - T2 readiness waits are compared by their exit status only
  - M5 shape leaves a declared live host unit listing out

## Rounds

Only round 4 counts. Rounds 1–3 were stopped and retired. Their failures were
recorded, never relabelled, and their raw logs are kept under
`.parity/retired/m8d-round*/`.

| Round | Outcome | Cause |
| --- | --- | --- |
| 1 | retired | two T2 failures. See below. |
| 2 | retired | T2 Go/Go: a cluster readiness wait printed progress on one twin |
| 3 | retired | M5 shape 1509/1510. See below. |
| 4 | **pass** | |

**Round 1:**

- **T2 Go/Go, `host-domain.archives.extract` #12:** one twin listed the
  driver's own ssh login session scope at the baseline only.
- **T2 Go/Rust, `oci-domain.build.push-to-twin-registry` #1:**
  `podman-auto-update.timer`, enabled by the podman package, ran at the
  baseline on one twin. The golden was rebuilt with the timers disabled and
  pinned anew.

**Round 2:** `cluster-domain.agent.vm-install` #8. The setup's
`kubectl rollout status` printed "Waiting for deployment" lines on one twin
only. Whether kubectl reports progress depends on twin timing.

**Round 3:** M5 shape failed on `provider-lifecycle.teardown.idempotent`
#3. Its step `teardownNoTarget` resolves the provider's unit by listing the
developer host's systemd units. Go and Rust both run `/usr/bin/systemctl` by
its absolute path, which no PATH shim reaches. The D-Bus activated
`NetworkManager-dispatcher` started between the two sides' listings, so
Rust registered 220 units to Go's 219. The difference came from the host;
the other four iterations matched at 219/219.

## Open, owned gaps

These come from [gaps.json](../../baseline/inventory/gaps.json): 19 gaps, 0
unowned. No M8d gap remains:

- The `podman` and `buildah` effect tiers are `T2-sandbox`.
- Every OCI T2 inventory item is covered.

| Owner | Gaps |
| --- | --- |
| M8a | 3: `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution`, which need a WSL2 runner |
| M8f | 2 |
| M10 | 1 |
| M8, environment reads for later domains | 13 |
