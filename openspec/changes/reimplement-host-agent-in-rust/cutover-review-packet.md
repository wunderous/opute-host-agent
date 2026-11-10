# Cutover review packet: Rust 0.3.0 release candidate

For task 6.3: the Host Agent, Platform, release and deployment owners review
one release candidate and the evidence that names it. This packet selects
nothing and authorizes nothing. Go v0.2.4 stays the production owner until
the owners sign below and the responsible owner authorizes cutover through
its normal rollout path (spec, *Cutover requires specification-based
evidence*). Tasks 6.3 and 6.4 stay open.

Machine-checked form: `make cutover-readiness` (`evidence/cutover/readiness.py`,
items in `evidence/cutover/readiness.json`). Its `candidate` item fails
unless every gate below, the contract receipt and the arm64 receipt ran this
one release set and the Rust sources are unchanged since it was built.

## 1. The candidate

| Field | Value |
|---|---|
| Name | `rust-0.3.0-rc1` ([`evidence/cutover/candidate.json`](../../../evidence/cutover/candidate.json)) |
| Version | `0.3.0` (the binary's `--version`; D28 makes 0.3.0 the first Rust package) |
| Commit | `74b3d2e827f335b9e7bf58c27f92f8f5f5857c94` (main after PR #45) |
| Built | locally with `tools/release/build-artifacts.sh`, `VERSION=0.3.0`; no tag, no publish |
| `SHA256SUMS` | `sha256:07f35fb61b4f4ab1189b545f73d6e585fee2145bf5feff7924daad3ccf257e63` |
| `host-agent-linux-x64` | `304013dfde432e89a414622f62991b11596ef8a5b2872c639a6835da9a374828` |
| `host-agent-linux-x64.gz` | `d3edf9eaf4dcc0d9c9456abcb97246e8054adcfd5e29e4e612e11b5593035abe` |
| `host-agent-linux-arm64.gz` | `3cec9874e4ec795fae53a2c85a7e4f40d60e3cfdd65a74300db087b5cc414530` |
| `opute-provider-k3s-linux-x64` | `cdf421a137cbb772d0c39117c8a263dc67ff64c5685b96e862cb2d2c95b6cfad` |
| `opute-provider-cloudflare-linux-x64` | `9ab3dfd41a0342ba95951b100611cba4d62f0d66407c10835909f38b5af5e038` |
| `opute-provider-tailscale-linux-x64` | `6113fa62b4ae0dd4c47fc85d1480654901e5c899366bb8e793e112c099eea2e6` |
| Catalog revision, standalone | `sha256:90679cf13dc4434c27c9da3f9b681abc2dbf8d916af80b1d22fe42b8d62709bb` (142 public tools; the site capture from the binary reports the same revision) |
| Catalog revision, platform | `sha256:3629012092dd06e499ff59ee968006f075de9e0513fa7e7d571828f57df4f54e` |
| Upgrade target | `rust-0.3.0-rc1-next`, `0.3.1-e2e`, the same commit, used only by the in-place upgrade journeys |

The dev VM was rebuilt from this set; its build refuses unless the installed
binary reports `0.3.0`. Go stays the published v0.2.4 release
(`baseline/source-lock.json`).

## 2. Gates and receipts

Every gate ran on the candidate under one run id, `m13b-r1`
(`evidence/e2e/runs/m13b-r1/`), and `evidence/e2e/verify.py --gate` passes
each. Each journey names the spec requirements it proves in its
`results.json`.

| Gate | Journeys | Spec requirements passed | What it proves |
|---|---|---|---|
| e2e-0 | 7 | 7 | The harness can fail: undeclared effects, a broken pass condition and a schema violation are reported as failures, and the VM restores between journeys |
| e2e-1 | 8 | 20 | Install and first success: the system and user units, the npm launcher (packed locally), tutorial clients, auth and admission refusals with no durable writes, two agents and the writer lock, restart recovery, and migration from Go v0.2.4 with an in-place upgrade |
| e2e-2 | 5 | 30 | Typed domain work: host operations, Incus containers and VMs with ownership, cross-domain plans and cancellation, OCI build/push/run on K3s, PostgreSQL and SQLite data services |
| e2e-3a | 5 | 17 | A three-server HA K3s cluster built only through typed calls: workload, losing one server, quorum loss with fail-closed calls and recovery, and the optional cluster endpoint (Q19) |
| e2e-3 | 3 | 15 | Use cases: cluster discovery through the neutral name, the site deploy recipe, local model serving through the gateway (a test runtime) |
| e2e-4 | 2 | 10 | Cutover: Rust on Go v0.2.4's real state with work in flight, Rust's new work, rollback to Go on Rust's state; SIGTERM, SIGKILL and an in-place upgrade mid-work |

Two journeys of the first pass (`e2e1.migrate-upgrade`, `e2e4.cutover`)
failed one harness assertion each: they expected the installed Rust binary
to print `0.2.4`. The generators now take the version from the candidate,
and both reran green under the same run id. No product change; the failed
results and their cause are retired under
`.parity/retired/m13b-r1-version-pin/`.

| Receipt | Where | What it proves |
|---|---|---|
| Contract suites | [`contract-suites.json`](../../../evidence/cutover/contract-suites.json), binary `304013df…` | D8 OAuth issuance 24/24, D10 standalone read-only gate 8/8, D11 rejected calls leave no audit writes 3/3, D12 refusal before an operation record 3/3, D13 evidence redaction 4/4 |
| Seam report | [`evidence/seam/m13b`](../../../evidence/seam/m13b) and its validation | D14: 133 compliant capabilities, 17 unported, no gap; the dispatch canary turns the verifier red |
| arm64 clean install | [`arm64/receipt.json`](../../../evidence/cutover/arm64/receipt.json), `host-agent-linux-arm64.gz` `3cec9874…` | Debian 12 arm64 under emulation: checksum, install, launch, MCP `tools/list` and `get_host_info` with the exact agent id, SIGTERM stop, relaunch, stop, uninstall |
| Distribution | [`distribution.json`](../../../evidence/cutover/distribution.json) | Go's published-package read-only canary passes (1/1) on the locally packed 0.3.0 launcher starting the candidate; the site preview from the candidate's catalog equals Go's after the declared renames except in exactly the four tools D28 changed (below); Go's own `check-site` passes from Go v0.2.4 |

The distribution checks map onto tasks 5.1 and 5.2 as follows. All are local.

| Check | Evidence |
|---|---|
| Clean-machine install (x64) | `e2e1.system-unit`, `e2e1.user-unit-npm` (both in m13b-r1) |
| Clean-machine install (arm64) | the arm64 receipt |
| Upgrade from Go v0.2.4 | `e2e1.migrate-upgrade` and `e2e4.cutover` |
| Rollback to Go v0.2.4 | `e2e4.cutover` |
| npm launch path from a locally packed tarball | `e2e1.user-unit-npm`; the read-only canary in `distribution.json` |
| Site preview against the candidate catalog | `distribution.json`, `site` |

The candidate's catalog on the committed site fails Go's
`check_site_release_parity`: `site/context/release-catalog.json` is a decision
anchor and must be re-anchored explicitly. That is the 5.2 public-claim step
(M10 finding 1), not done here: no public claim changed.

Site preview tools that differ from Go after the D19–D24 renames, each
decided by the owner:

| Tool | Decision |
|---|---|
| `configure_service_domain`, `remove_service_domain` | D28 (Q28): publish the `uri` descriptor ([bind-standalone-service-domain](../bind-standalone-service-domain/proposal.md)) |
| `restart_host_service` | D28 (Q23): describe user services only ([privileged-host-service-restart](../privileged-host-service-restart/proposal.md)) |
| `provision_container` | D28 (Q21): declare the inputs it reads ([declare-read-inputs](../declare-read-inputs/proposal.md)) |

## 3. Declared divergences from Go

Every behaviour in which the candidate differs from Go v0.2.4 by decision.
Full text: the decision table in [milestones.md](milestones.md) and the
linked changes.

| D | Divergence | Decided |
|---|---|---|
| D3 | `host-agent-windows-x64.gz` is not a native execution target (the spec limits native execution to Linux and WSL2); recorded as a parity item | Recorded, owner |
| D8 | OAuth token issuance follows the stricter `oauth-issuance` contract | owner, 2026-10-01 |
| D9 | go-sdk `MCPGODEBUG` switches are not reproduced; Rust behaves as Go with the variable unset | Not reproduced, owner |
| D10 | With standalone mutations disabled, only `read`-effect tools run | owner, 2026-10-02 |
| D11 | Enumerated invalid requests and the closed standalone gate leave no audit writes | owner, 2026-10-02 |
| D12 | A task-aware call refused at binding or admission writes no operation record | owner, 2026-10-02 |
| D13 | Unmarked durable fields follow the stricter projection contract (two named policies) | owner, 2026-10-04 |
| D16 | A failed provider candidate does not displace other providers' family operations | owner, 2026-10-12 |
| D17 | A null provider input-request entry is refused with a decode error; the agent keeps serving | owner, 2026-10-12 |
| D18 | A delegated callback child that never starts leaves no durable row (extends D12) | owner, 2026-10-12 |
| D19, D21, D22 | Provider-neutral host, instance and cluster tool names, with Go aliases | owner, 2026-10-12/14/16 |
| D23 | Provider-neutral OCI selector values | owner, 2026-10-17 |
| D20 | A corrupt zstd block fails as Go does, with declared error text; the decoder is not ported | owner, 2026-10-12 |
| D24 | The PostgreSQL relay token is published `writeOnly` | owner, 2026-10-19 |
| D25 | One writer per instance state: a second agent on the same state is refused (Go and Rust) | owner, 2026-11-01 |
| D26 | hostos, cloudflare, tailscale and k3s providers are Rust executables; ollama stays out (D15) | owner, 2026-11-02 |
| D27 | The spec, not Go, is the authority after the parity record | owner, 2026-11-04 |
| D28 | Q0–Q28 answered: the fixes in Q5, Q7, Q9, Q14–Q17, Q20–Q28; the 14-day rollback window; 0.3.0 as the first Rust package; npm postponed | owner, 2026-11-07 |
| D28 follow-ups | Q19 (optional HA endpoint, off by default) and Q29 (one workload unit per task) | owner, 2026-11-08 |

D1 (bit-compatible SQLite), D4–D7, D14 and D15 constrain how the migration
was done or scoped; they change no observable behaviour against Go.

## 4. Not passed, and who unblocks it

| Item | State | Owner | Unblocked by |
|---|---|---|---|
| e2e-5 | blocked | Opute Platform owners | a Platform staging environment with an enrollment identity for the agent (D5); task 5.3 |
| WSL2 | blocked | Opute release owners | a Windows host with WSL2 as a runner (5.1) |
| Q4 | unverified | Opute Platform owners | the owner confirms whether Platform stores hashes |
| Q18 | unverified (deferred) | Host Agent owner | D28 defers it as a new capability; not a cutover blocker |
| npm | postponed | release owners | D28: publishing 0.3.0, including to npm, is a separate rollout step; nothing here was published |
| Public site and catalog claims (5.2) | not started | release and website owners | re-anchoring `site/context/release-catalog.json` on the candidate's catalog at publication |

`make cutover-readiness` lists exactly e2e-5, WSL2, Q4 and Q18 as not passed.

## 5. Cutover and rollback runbook

For the deployment owner, after sign-off. Each step names the journey that
rehearsed it on the dev VM with the candidate (run `m13b-r1`). Production
hosts, Platform registration, tunnels and DNS were never touched.

| # | Step | Rehearsed by |
|---|---|---|
| 1 | Verify the release set against `SHA256SUMS` before installing | `e2e1.system-unit` (artifact check), arm64 receipt |
| 2 | On the Go host, keep the unit, binary path, instance root, env file and `OPUTE_REMOTE_AGENT_ID` unchanged | `e2e1.migrate-upgrade`, `e2e4.cutover` |
| 3 | Snapshot the instance state directory after Go stops | not on the candidate: the `m12a` gate (task 6.2) snapshotted isolated copies on an earlier build. `e2e4.cutover` rolls back on Rust's live state without a snapshot |
| 4 | Stop Go with SIGTERM; in-flight policy is Go's: drain requests, cancel running tasks and wait (D12 for tasks still in binding) | `e2e4.cutover` (stopped with an open call, a working task and a running plan), `e2e1.restart` |
| 5 | Replace the binary with `host-agent-linux-x64` from the candidate; keep Go v0.2.4's binary on the host (selectable for 14 days) | `e2e1.migrate-upgrade`, `e2e4.cutover` |
| 6 | Start Rust on Go's state; it reconciles per table; read back operations, tasks and plan runs | `e2e4.cutover` |
| 7 | Confirm the writer lock: no second agent on the same state | `e2e1.two-agents`, `e2e4.cutover` (D25) |
| 8 | Confirm first success with the tutorial clients and the exact agent id | `e2e1.tutorial-clients`, `e2e1.auth` |
| 9 | Observe through the window: restart and kill mid-work recover truthfully | `e2e1.restart`, `e2e4.restart` |
| 10 | In-place Rust upgrade during the window | `e2e1.migrate-upgrade`, `e2e4.restart` (to `0.3.1-e2e`) |
| R1 | Rollback, any time within 14 days: stop Rust with SIGTERM, put Go v0.2.4's binary back, start Go on Rust's state; every record reports the status Rust last reported | `e2e4.cutover` |
| R2 | After 14 days with evidence green: a separate, evidenced decision to retire Go (task 6.4); nothing is removed by this packet | — |

## 6. Sign-off

Left empty for the owners (task 6.3).

| Owner | Name | Decision | Date | Notes |
|---|---|---|---|---|
| Host Agent | | | | |
| Opute Platform | | | | |
| Release | | | | |
| Deployment | | | | |
