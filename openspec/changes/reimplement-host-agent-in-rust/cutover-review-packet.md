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

m16 (hardening) replaced the m13b candidate `rust-0.3.0-rc1` with
`rust-0.3.0-rc4`: five defects found by fuzzing, the fault journeys and the
auth review were fixed (section 2a), and every gate reran on the new set.

## 1. The candidate

| Field | Value |
|---|---|
| Name | `rust-0.3.0-rc4` ([`evidence/cutover/candidate.json`](../../../evidence/cutover/candidate.json)) |
| Version | `0.3.0` (the binary's `--version`; D28 makes 0.3.0 the first Rust package) |
| Commit | `9c8ff2d4c90215b48e15149d2bcf64238b558d9d` (branch `claude/m16-hardening`, PR #47) |
| Built | locally with `tools/release/build-artifacts.sh`, `VERSION=0.3.0`; no tag, no publish. The set carries a CycloneDX SBOM, `sbom.cdx.json`, with its own checksum |
| `SHA256SUMS` | `sha256:82f21b9acaf5d6afa36bed445fa8b08d623d2068953736bacb5fc5ec8d0eb915` |
| `host-agent-linux-x64` | `994f06d4acc9e4dde989f790e1d6f6e113738efb5c5e053fc8fa4c037e1dc837` |
| `host-agent-linux-x64.gz` | `03645a70b8df8d3d99f679228a1ad0f5866c573a2a56ceb5441020ecdc553973` |
| `host-agent-linux-arm64.gz` | `fef25445cc428ff66e7db5f4bd2c8be2010b4cd4d60fbf7704ef8b3c9f019781` |
| `opute-provider-k3s-linux-x64` | `78090752df867a2b3342d291c6e5a91b5dbc34e07ccf9c33ec4c9472a77ec950` |
| `opute-provider-cloudflare-linux-x64` | `c4a5c7b306324763a7d81b11dbc65d8191832f16d64b9c6b5e54593ee692a98f` |
| `opute-provider-tailscale-linux-x64` | `324fecb075ad5ba8b1b2d3a02e2ca74c505464fe94a7c6680847a37c700668af` |
| Catalog revision, standalone | `sha256:90679cf13dc4434c27c9da3f9b681abc2dbf8d916af80b1d22fe42b8d62709bb` (142 public tools; the site capture from the binary reports the same revision) |
| Catalog revision, platform | `sha256:3629012092dd06e499ff59ee968006f075de9e0513fa7e7d571828f57df4f54e` |
| Upgrade target | `rust-0.3.0-rc4-next`, `0.3.1-e2e`, the same commit, used only by the in-place upgrade journeys |

The dev VM was rebuilt from this set; its build refuses unless the installed
binary reports `0.3.0`. Go stays the published v0.2.4 release
(`baseline/source-lock.json`).

## 2. Gates and receipts

Every gate ran on the candidate under one run id, `m16-r3`
(`evidence/e2e/runs/m16-r3/`), and `evidence/e2e/verify.py --gate` passes
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
| e2e-6 | 7 | 8 | Host faults and pressure (m16, D31): a full state directory refuses effectful calls and recovers without a restart; a truncated store stops startup and the restored store starts intact; SIGKILL during a plan step and twelve SIGKILLs with tasks in flight leave a whole store; a hung or garbage-printing `incus` ends the call typed and the agent serves on; a clock stepped back an hour keeps tokens and reservations; admission refuses beyond the operation limit and below the floors; a 1 GiB declared body is answered 413 (D29); the 15-minute resource budget holds |

Two earlier runs were retired with their cause: `m16-r1` (rc2) when fuzzing
found F5, and `m16-r2` (rc3) when CI's `cargo fmt` check failed and formatting
changed the candidate's source digest (`.parity/retired/m16-r1/`, `m16-r2/`).
In `m16-r3`, the first attempt of `e2e3a.lose-one` was killed by the round's
own 1780 s runner limit before writing any record, and the journey reran once
under the same run id (`.parity/retired/m16-r3-lose-one-timeout/`).

| Receipt | Where | What it proves |
|---|---|---|
| Contract suites | [`contract-suites.json`](../../../evidence/cutover/contract-suites.json), binary `994f06d4…` | D8 OAuth issuance 24/24, D10 standalone read-only gate 8/8, D11 rejected calls leave no audit writes 3/3, D12 refusal before an operation record 3/3, D13 evidence redaction 4/4 |
| Seam report | [`evidence/seam/m16`](../../../evidence/seam/m16) and its validation | D14: 133 compliant capabilities, 17 unported, no gap; the dispatch canary turns the verifier red |
| arm64 clean install | [`arm64/receipt.json`](../../../evidence/cutover/arm64/receipt.json), `host-agent-linux-arm64.gz` `fef25445…` | Debian 12 arm64 under emulation: checksum, install, launch, MCP `tools/list` and `get_host_info` with the exact agent id, SIGTERM stop, relaunch, stop, uninstall |
| Reproducible build | `make reproducible-build` (CI job `supply-chain`) | Two clean, offline, `--locked` builds of the agent and the four provider executables are byte-identical |
| Supply chain | `deny.toml`, `make supply-chain` (CI) | cargo-deny: advisories, licences, sources (crates.io only), duplicate versions; exceptions carry their reason (one accepted advisory, Q30) |
| Distribution | [`distribution.json`](../../../evidence/cutover/distribution.json) | Go's published-package read-only canary passes (1/1) on the locally packed 0.3.0 launcher starting the candidate; the site preview from the candidate's catalog equals Go's after the declared renames except in exactly the four tools D28 changed (below); Go's own `check-site` passes from Go v0.2.4 |

The distribution checks map onto tasks 5.1 and 5.2 as follows. All are local.

| Check | Evidence |
|---|---|
| Clean-machine install (x64) | `e2e1.system-unit`, `e2e1.user-unit-npm` (both in m16-r3) |
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

## 2a. Hardening (m16)

The evidence is under [`evidence/hardening/`](../../../evidence/hardening/README.md).
The inventory lists 22 entry points where untrusted bytes enter the agent;
each is covered by fuzz targets (17, cargo-fuzz on the stable toolchain) or
property tests (proptest), and a unit test fails if an entry loses its
coverage. CI fuzzes every target for 20 s; three local passes ran 60 s per
target, after a first was retired (results in the README, including the
targets that found nothing).

| Id | Found by | What | State |
|---|---|---|---|
| F1 | property test | A resource id with Unicode whitespace changed identity on a round trip | fixed, D30 |
| F2 | fuzz `http_wire` | An `Authorization` header with a multi-byte character panicked `/oauth/token`'s connection | fixed |
| A1 | auth review | `/mcp` read the whole body before the 4 MiB check | fixed, D29 |
| F3 | e2e-6 draft | With the state directory full, an effectful call ran and its audit row was silently lost | fixed under D31: refused with `state_storage_full` below 1 MiB free |
| F4 | e2e-6 draft | `/health` ran `incus list` on the thread serving every connection; a hung `incus` stalled the agent | fixed |
| F5 | fuzz `json_document` | A `\u` escape with a sign was accepted (`from_str_radix`); three more decodes had the same defect | fixed |
| A2 | auth review | The model-serving relay forwards a request body without a bound | open observation |
| A3 | auth review | The bootstrap bearer's local-`Host` rule limits audience, not which peer may use it | as specified |
| O1 | e2e-6 budget | Audit rows are never pruned: about 3.5 KB per call, 78 MB in 15 minutes of heavy load | open: retention is not specified |

The full auth walk is [`auth-review.md`](../../../evidence/hardening/auth-review.md).

Resource budget (`resource-budget.json`), on the candidate in `e2e6.budget`:
27,390 calls in 15 minutes with no error; after a two-minute warm-up RSS
grew 15.9 MB (budget 48 MiB), open FDs and threads did not grow, and the
store grew 3,455 bytes per call (budget 8 KiB; the first budget's absolute
64 MiB store limit had no measured basis and was replaced). Peak RSS 95 MB
(budget 256 MiB). The D29 check: a 1 GiB declared body was answered 413
at once, with RSS 43 MB afterwards.

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
| D29 | `/mcp` reads at most 4 MiB of a request body and answers 413 beyond it | owner, 2026-11-10 |
| D30 | A resource id containing any Unicode whitespace is refused | owner, 2026-11-10 |
| D31 | The outcomes of host faults (full state directory, damaged store, failing command, clock step) | owner, 2026-11-10 |

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
| O1 audit retention, A2 relay body | open observations | Host Agent owner | a decision on audit retention and on a relay body bound; neither is a cutover blocker by the spec |

`make cutover-readiness` lists exactly e2e-5, WSL2, Q4 and Q18 as not passed.

## 5. Cutover and rollback runbook

For the deployment owner, after sign-off. Each step names the journey that
rehearsed it on the dev VM with the candidate (run `m16-r3`). Production
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

## 7. Rust 0.4.0 candidate (goal m15)

Sections 1 to 6 are the 0.3.0 candidate, and its sign-off stands alone.
This section is the next candidate, from the integration branch
`rust-0.4.0`, which reaches main only after the 0.3.0 sign-off. Nothing
here was tagged or published. On `rust-0.4.0`, `evidence/cutover/candidate.json`
names this set and `make cutover-readiness` checks it.

### 7.1 What changed from 0.3.0

Every tool the agent publishes now either works or is withdrawn by
decision. The 0.3.0 seam report listed 17 unported capabilities; the 0.4.0
report (`evidence/seam/m15`) lists 144 compliant, 2 unported and 4
unpublished, with no gap. The two unported ones are internal
(`configure_host_network`, `install_sql_forward_sidecar`) and published in
no mode. The dispatch canary turns the verifier red.

| Tools | Change | Decision |
|---|---|---|
| `list_secret_keys` | Key names of one Secret, never values | D32 (Q18), [implement-list-secret-keys](../implement-list-secret-keys/proposal.md) |
| `list_configmap_keys` | Key names of one ConfigMap | D33, [list-configmap-keys](../list-configmap-keys/proposal.md) |
| `inspect_workload` | A fixed projection of one Deployment: env names and references, never values | D34, [inspect-workload](../inspect-workload/proposal.md) |
| `configure_network` | One bridged NIC on an instance | D35, [vm-bridged-nic](../vm-bridged-nic/proposal.md) |
| `remove_vm_network_device` | Remove one NIC from the instance's own configuration | D36, [remove-vm-network-device](../remove-vm-network-device/proposal.md) |
| `register_kubernetes_cluster`, `list_agents`, `install_provider_tools`, `uninstall_provider_tools` | Withdrawn from the catalog | D37, [withdraw-platform-and-legacy-provider-tools](../withdraw-platform-and-legacy-provider-tools/proposal.md) |
| `stream_vm_console`, `send_console_input`, `resize_console` | The serial console (`incus console`) on a PTY; output pulled from a 64 KiB buffer in `send_console_input`'s new `output` and `closed` | D38, D39, [vm-console-session](../vm-console-session/proposal.md) |
| `open_assistant_session`, `remove_local_llm_model`, `configure_local_llm_runtime` | As recorded in v0.2.4 (D27) | [recorded-session-and-llm-tools](../recorded-session-and-llm-tools/proposal.md) |
| `--mode`, Platform defaults and names | Removed. `OPUTE_HOST_ALLOW_MUTATIONS` gates mutations | [platform-neutral-host-agent](../platform-neutral-host-agent/proposal.md) (PR #43) |

The catalog revision for standalone is re-pinned for D37 and D39 to
`sha256:26a771cc7f2b2a7b9a78ad3e116ac3e44c8a07d1cdc98bec55de02d00202a27a`.

### 7.2 The candidate

| Field | Value |
|---|---|
| Name | `rust-0.4.0-rc1`, version `0.4.0` |
| Commit | `3b3b6b7f8b31958a60a440c8f8f2ffb617ca7030` (the merge of PR #48 into `rust-0.4.0`) |
| `SHA256SUMS` | `sha256:f85bb387a9757b3660a6c9db74350b0d650bbdc4e3e643cd9207df391773468d` |
| `host-agent-linux-x64` | `3e966602c3de67d909a0f04642a81d4b81392341592ce3ffe28dad944fc93462` |
| `host-agent-linux-arm64.gz` | `19f5a71296ad8d2a6444238f2d8a1dd01251a88bcd58d6fd2b9108040d6bff38` |
| Upgrade target | `rust-0.4.0-rc1-next`, `0.4.1-e2e`, the same commit |
| Rollback target | `rust-0.3.0-rc4` (section 1), used by `neutral.rollback` |

Before the build: `cargo fmt` and clippy were clean, and a 60 s pass over all
18 fuzz targets found nothing. The 23 hardening entry points include the
new console-output reader (fuzz target `console_output` and a property test).

### 7.3 Gates and receipts

Every gate ran on rc1 under run `m15-r1`, and `evidence/e2e/verify.py --gate`
passes each.

| Gate | Journeys | Spec requirements passed | New in 0.4.0 |
|---|---|---|---|
| e2e-0 | 7 | 7 | — |
| e2e-1 | 8 | 35 | Install on the proposed unit and launcher (below); upgrade to `0.4.1-e2e` |
| e2e-2 | 5 | 38 | `e2e2.incus`: the console journey (open, read the login prompt, resize, detach, no `incus console` left) and the NIC journey (attach, attach again, remove, remove again) |
| e2e-3a | 5 | 33 | `e2e3a.workload`: cluster inspection on the HA cluster through `list_secret_keys`, `list_configmap_keys` and `inspect_workload`; the Q18 workaround is gone |
| e2e-3 | 3 | 18 | `e2e3.serving` calls both LLM tools; `e2e3.discovery` reads K3s's own keys |
| e2e-4 | 2 | 11 | — |
| e2e-6 | 7 | 13 | — |
| e2e-neutral | 3 | 13 | Rerun after PR #43's merge; rollback from 0.4.0 to 0.3.0 rc4 |

Retired, with cause (`.parity/retired/`):
- `m15-r1-stale-journeys`: the first attempts of `e2e1.migrate-upgrade`
  and `e2e4.restart` expected the version `0.3.1-e2e`, and `e2e3.discovery`
  expected `not_implemented` from the two key tools. These were journey
  expectations left from 0.3.0, not product defects.
- `m15-r1-neutral-reserved-provider`: the first attempt of `neutral.reserved`
  passed on another build's Cloudflare provider.

Each of these journeys reran once under the same run, on the same build.

| Receipt | What it proves |
|---|---|
| Contract suites | Binary `3e966602…`. D8 OAuth issuance passes 23/23, D10 7/7, D11 3/3, D12 3/3 and D13 4/4. PR #43 (platform-neutral) removed two 0.3.0 scenarios. `credentials.operator-secret` went because OAuth machine clients now come from `OPUTE_OAUTH_CLIENTS`. `gate.platform-unaffected` went because there is one serving profile and no mode |
| Seam `m15` | The counts above, the red canary, and a rerun that reproduces the report byte for byte |
| arm64 clean install | `host-agent-linux-arm64.gz` `19f5a712…` |
| Distribution | Go's read-only canary passes on the locally packed 0.4.0 launcher. After the declared renames, the site preview differs from Go's only in the tools D28, D37 and D39 changed and in `run_host_local_recipe`'s description, from which platform-neutral removes the Platform name |

### 7.4 Prerequisite for shipping 0.4.0: the proposed install surfaces

0.4.0 refuses `--mode`. Go's deploy unit, the npm launcher and the site's
catalog capture still pass it. The owners decided on 2026-11-11 that the
0.4.0 gates run on the replacements proposed in
[`evidence/cutover/proposed-0.4.0/`](../../../evidence/cutover/proposed-0.4.0/README.md):

- the unit without `--mode` and with `OPUTE_HOST_ALLOW_MUTATIONS=true`;
- the launcher patch;
- the site capture patch.

The release owners still ship these files. Until they do, 0.4.0 cannot
start from the published unit or launcher (platform-neutral-host-agent task
4.0 stays open). Section 5's runbook applies to 0.4.0 with two changes.
In step 5, install the proposed unit with the binary. R1 rolls back to the
0.3.x binary on the same unit: both settings sets stay in it, and 0.3.x
reads its old ones (`neutral.rollback`).

### 7.5 Not passed

`make cutover-readiness` on `rust-0.4.0` lists e2e-5, WSL2 and Q4, the same
owners and unblockers as section 4. Q18 is answered by D32 and passes.
