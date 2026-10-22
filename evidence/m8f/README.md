# M8f: llm and serving domains (evidence)

M8f ports the Go `internal/domain/llm` and `internal/domain/serving`
operations to Rust as built-in providers of seven three-role services (D14):

- **local llm runtime:** `check_local_llm_prerequisites`,
  `configure_local_llm_model`, `install_local_llm_model`,
  `list_local_llm_models`, `probe_local_llm`, `start_local_llm_runtime`,
  `stop_local_llm_runtime`, for the `ollama` and `llama-cpp` runtimes
- **local llm server binary:** `ensure_local_llm_server_binary`
- **local llm relay:** `ensure_local_llm_relay`, `remove_local_llm_relay`,
  persisted and restored
- **local llm cluster proxy:** `ensure_local_llm_k3s_proxy`,
  `remove_local_llm_k3s_proxy`
- **openai-compatible probe:** `probe_openai_compatible_server`
- **serving assignment:** `reconcile_serving_assignment`
- **service ingress:** `discover_service_ingress`

The tool names and runtime selector values stay Go's ([NOTE.md](NOTE.md)).

Parity is proven at two tiers:

- **T1:** recording shims and a route-based HTTP stub, 13 llm- and
  serving-domain scenarios.
- **T2:** disposable twin VMs running a CPU-only model stub as `ollama`,
  `llama-server` and `nvidia-smi`, real `systemd --user` units, a real
  relay and a real transient serving unit.

The Go authority is `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`). The
Rust candidate is `60587cec…`, built with `cargo build --release --locked`
from round head `94b790e3`. M8f branches from `main` after PR
wunderous/opute-host-agent#19 merged.

**Result:** the m8f gate passes, and gates m0–m6 and m8a–m8e still pass.
The cutover gate fails as expected, on 4 owned inventory gaps and the
cutover evidence files that belong to later milestones. All 13 llm- and
serving-domain items and the four llm and serving T2 scenarios moved from
`unverified` to `pass` in both suites.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (every command) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| T1 Go/Go ×20 and Go/Rust ×5, all 327 manifest items | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 Go/Go ×20 and Go/Rust ×5, 34 scenarios (14 host, 5 incus, 5 cluster, 3 oci, 3 postgres, 3 llm, 1 serving) | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), goldens: [base](t2-golden.json), [incus](t2-golden-incus.json), [k3s](t2-golden-k3s.json), [oci](t2-golden-oci.json), [postgres](t2-golden-postgres.json) |
| Go `test/live` oracle inside the twins | [oracles-t2.json](oracles-t2.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [m8d](verify-m8d.json) [m8e](verify-m8e.json) [m8f](verify-m8f.json) [cutover](verify-cutover.json) |
| Contracts, canaries and client oracles | [contracts/](contracts/), [Go canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [go-sdk OAuth client](oauth-sdk-client/receipt.json) |
| M5 durable state, including the secret sweep (16 Go fields, 8 D24 fields) | [m5/](m5/) |
| D14 seam stage | [report](../seam/m8f/report.json) |
| Entry, reachability, Go quirks and the credential finding | [NOTE.md](NOTE.md) |
| Inventory | [llm domain](../../baseline/inventory/llm-domain.json), [serving domain](../../baseline/inventory/serving-domain.json), [env](../../baseline/inventory/env.json), [effects](../../baseline/inventory/effects.json), [t2](../../baseline/inventory/t2.json), [gaps](../../baseline/inventory/gaps.json) |

## What passed in round 3

All 31 steps exited 0. The tree was clean before and after, and the binaries
did not change during the round.

| Check | Result |
| --- | --- |
| fmt, Clippy `-D warnings`, workspace tests, locked release build | pass; 253 tests |
| `make spec-validate`, catalog source, harness tests | pass; 18 specs |
| Owning Go tests: `internal/domain/llm` and `internal/domain/serving` ×5; core packages | pass; 270 owning passes, 0 failures |
| T1 Go/Go ×20 | 327/327 |
| T1 Go/Rust ×5 | 327/327 |
| T2 Go/Go ×20 | 34/34 |
| T2 Go/Rust ×5 | 34/34 |
| Go `test/live` oracle in a twin | Rust's per-test outcome equals Go's; negative control passes nothing |
| Go canaries / Rust canaries | 14/14 caught / 24/24 caught |
| Contracts D8, D10, D11, D12, D13 | 24/24, 8/8, 3/3, 3/3, 4/4 |
| M5 crash, shape, migration, cross-read, unknown projection | 200/200, 1620/1620, 6/6, 18/18, 0 failures |
| Secret sweep | 16 Go-marked fields on both sides, 8 D24 fields on Rust; 0 failures |
| Seam verifier | 130 compliant, 20 unported, **0 gaps**; dispatch canary red |

## T1

[`tools/parity/llm_domain_scenarios.py`](../../tools/parity/llm_domain_scenarios.py)
generates both matrices. `systemctl`, `nvidia-smi`, `ss`, `kill`,
`systemd-run` and `ollama` are recording shims. The runtimes' HTTP APIs are
answered by the harness's route stub, which records every request's method,
path, body and the headers Go sets. Unit files and configs are compared by
content.

- **ollama:** absent, the full lifecycle (configure, install, start, list,
  probe, stop), the unit with and without a models directory, and a unit
  start that fails.
- **llama-cpp:** the lifecycle from a manifest, an admission refusal, and
  the server's environment.
- **relay:** ensure, restore and remove, with Go's token fallback.
- **cluster proxy:** Go's refusal (see Reachability).
- **openai probe:** a streaming chat answered gzip-encoded, as Go asks for.
- **serving:** an exclusive assignment's lifecycle with a fixed id, its
  refusals, and ingress discovery through the k3s provider.

## T2 sandbox

All four scenarios boot from the base golden with 6 GiB twins. The heavy
admission class costs 2 cores and 2 GiB, so a 2 GiB twin refuses
`host_capacity_saturated`. The model stub
([`t2guest/llmstub.py`](../../tools/parity/t2guest/llmstub.py)) serves
`ollama serve|pull`, `llama-server` and `nvidia-smi` from a JSON model store,
on the CPU, so no model is downloaded.

- `llm-domain.ollama.lifecycle`: a real `systemd --user` unit runs the stub
  as `ollama serve`. Prerequisites, install, list, configure (persisted,
  then a context size), probe, start and stop; then a relay to it, an OpenAI
  probe through the relay, and the relay removed.
- `llm-domain.llama.lifecycle`: install and probe, the server binary
  ensured twice (adopted, then through the toolchain) from a CUDA-enabled
  manifest with the stub `nvidia-smi` on the path, then start and stop.
- `llm-domain.admission.rejected-no-effect`: reruns every llm and serving
  call that M4 refuses (gate closed, stale revision). Its delta stays empty
  on both twins.
- `serving-domain.assignment.lifecycle`: a real transient unit serving HTTP,
  reconciled four times: launch, again, a new generation, and steady.

No shared Incus, Kubernetes, registry, tunnel or deployment state was
touched.

## Reachability

Go's `ensure_local_llm_k3s_proxy` and `remove_local_llm_k3s_proxy` take
their VM from a resource binding their catalog entries never declare, so
every call refuses with `vmName is invalid`. Rust reproduces the refusal
(T1 `llm-domain.cluster-proxy`, the T2 rejected rerun), and the T2
inventory lists both inert.

## Go behaviour reproduced as found

[NOTE.md](NOTE.md) lists them: the Ollama unit's slice aliasing when a models
directory is set, a failed `ollama pull` that is not an error,
`systemdUserAvailable` always true, the serving launch's own error dropped,
and map order (scenarios give at most one invalid field).

## Finding for the owner: relay credentials

Go does not mark the relay's `incomingToken`, `upstreamToken` and
`relayToken`, or the cluster proxy's `relayToken` and `bearerKey`,
`writeOnly`. Go's projection therefore keeps them verbatim in
`capability_invocations.arguments_json`, and Rust does the same. No owner
decision covers them, so none was invented ([NOTE.md](NOTE.md)). A
`writeOnly` mark would follow D24's shape in its own OpenSpec change.

## Defects fixed while proving parity

- **Rust:** the streaming chat probe asks for and decodes gzip, as Go's
  transport does; the Ollama unit reproduces Go's slice aliasing.
- **Harness:** each fix is committed separately.
  - a route-based HTTP stub, numeric run variables, requests to another
    listener, and file removal
  - a T2 scenario may ask for more twin memory
  - the `ollama.unit` port is masked where M5 reads its durable rows
    (round 1)
  - the harness's own `psql` client's stderr is masked like the incus
    client's (round 2)

## Rounds

Only round 3 counts. Rounds 1 and 2 were stopped or failed and were retired.
Their failures were recorded, never relabelled, and their raw logs are kept
under `.parity/retired/m8f-round*/`.

| Round | Outcome | Cause |
| --- | --- | --- |
| 1 | retired | M5 shape, `llm-domain.ollama.unit`. See below. |
| 2 | retired | T2 Go/Go, `t2.cluster-domain.storage.prune-and-collect` #2 and `t2.postgres-domain.service.lifecycle` #16. See below. |
| 3 | **pass** | |

**Round 1:** the scenario's stub port is a run variable. T1 masked it in the
wire result but not in the durable rows M5 shape reads back
(`capability_invocations.arguments_json`, `operations.task_snapshot_json`).
Both now carry the mask; shape was 65/65 on the M8f scenarios before round 2.
The round was stopped during T2 Go/Go.

**Round 2:** two earlier scenarios failed once each in Go/Go, under host
load:

- In one Go twin, containerd hit `DeadlineExceeded` removing an exited
  container, so Go's prune reported 2 removed and a warning, not 3. This is
  Go's own result, so nothing is masked.
- The harness's `kubectl exec` write step printed client-go's
  `Unknown stream id 1` websocket warning. That client's stderr is now
  masked; its exit status and stdout stay compared.

Both scenarios then passed Go/Go ×20 alone before round 3.

## Open, owned gaps

These come from [gaps.json](../../baseline/inventory/gaps.json): 4 gaps, 0
unowned. No M8 gap remains:

- Every llm and serving tool is covered, or inert where Go never binds.
- The 12 llm-domain environment variables, and the `ollama` and `kill`
  effects, are covered.

| Owner | Gaps |
| --- | --- |
| M8a | 3: `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution`, which need a WSL2 runner |
| M10 | 1: distribution (clean-machine installs, npm launcher, release artifacts) |
