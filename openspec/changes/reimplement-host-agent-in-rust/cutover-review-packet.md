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

m18 replaced `rust-0.3.0-rc4` with `rust-0.3.0-rc5`. Two findings first
recorded against 0.4.0 were measured on rc4 and were present there: every
tool the agent ran received its bearer and any provider credential (A4),
and a provider executable's `/mcp` read an oversized body whole (A5), as the
serving relay did (A2). D43 applies their 0.4.0 decisions, D40 and D41, to
0.3.0 (section 2a). Every gate reran on rc5.

m19 replaced `rust-0.3.0-rc5` with `rust-0.3.0-rc6`. A sweep of where each
secret the agent holds goes (argv, file modes, the journal, the store,
responses) found six paths off their place on rc5; D46 states the rules and
D47 to D51 fix them (section 2a). Every gate reran on rc6.

m21 replaced `rust-0.3.0-rc6` with `rust-0.3.0-rc7`. A journey along the
release path (Go v0.2.4, then 0.3.0, then 0.4.0, then a rollback to 0.3.0)
found that 0.3.0's public-MCP connector tools always failed their origin
probe: they asked the agent's own token endpoint for a token without a
secret, which D8 refuses (U1). D52 makes the probe mint its token in process,
as D8 requires (section 2a). Every gate reran on rc7.

m22 kept `rust-0.3.0-rc7`. U1 had hidden because a journey accepted a
failed call. The harness now fails a journey with `unchecked-outcome` when a
tool call or task poll has no assertion deciding its outcome, or returns an
error no assertion expects (`evidence/e2e/README.md`, *Outcomes*). Every
flagged step was classified (section 2a): one journey defect, four expected
refusals, no product defect. Every gate reran on rc7 under the rule.

m23 kept `rust-0.3.0-rc7`. An asserted success is still the tool's own
report. The harness now fails a journey with `unconfirmed-effect` when a
mutation it expects to succeed is not confirmed by a later, independent
observation that checks the effect (`evidence/e2e/README.md`, *Effects*).
All 217 unconfirmed steps on main are confirmed, or opt out with their
reason. Two journey defects were found and no product defect (section 2a).
Every gate reran on rc7 under both rules.

m24 kept `rust-0.3.0-rc7`. Nothing bounded what else a journey's effects
patterns let happen. The harness now fails a journey with `unused-allowance`
when an `allow` pattern matches no change of its own run, or a `persist`
pattern no residue, and with `broad-allowance` when a pattern covers a whole
tree (`evidence/e2e/README.md`, *Allowances*). On main, 122 of 331 allow
patterns and 14 of 24 persist patterns were unused, and 46 were broad; each
now names what its run changes. Five kinds of journey defect were found and
no product defect (section 2a). Every gate reran on rc7 under the three rules.

## 1. The candidate

| Field | Value |
|---|---|
| Name | `rust-0.3.0-rc7` ([`evidence/cutover/candidate.json`](../../../evidence/cutover/candidate.json)) |
| Version | `0.3.0` (the binary's `--version`; D28 makes 0.3.0 the first Rust package) |
| Commit | `cf151caa7b5b13b29f553d9645221ba734fa3264` (branch `claude/m21-self-probe`) |
| Built | locally with `tools/release/build-artifacts.sh`, `VERSION=0.3.0`; no tag, no publish. The set carries a CycloneDX SBOM, `sbom.cdx.json`, with its own checksum |
| `SHA256SUMS` | `sha256:e01d7bb8cfbb504fdd0b4940f5a05b9355534ee715892012da2f4b9d9be95df6` |
| `host-agent-linux-x64` | `f5b1175704c97b1a46f24be5a854b71c3375ac270468b52783683c5b4b260ff0` |
| `host-agent-linux-x64.gz` | `a8443363f9284f75be74adbb5499e978ab01c5363fb0acdc8258877d0b8f3da7` |
| `host-agent-linux-arm64.gz` | `b2396cc0e1860e30869415614cc7f29931e3126e6e160136a387b140380772cf` |
| `opute-provider-k3s-linux-x64` | `e9f8c3882b728fbd77b77bd216521d39bc3ceb022427f00a49709ed90254cfb4` |
| `opute-provider-cloudflare-linux-x64` | `9800ebfb72b44a53d89a1e605d9dd3b1f2ef46c245fac5a0d74acd7e263b6515` |
| `opute-provider-tailscale-linux-x64` | `3d78e0916fdede00c62b40fe9f64376ef5630f0973574938775958e731f44a1d` |
| Catalog revision, standalone | `sha256:9aba4c895695d25c6b8f91aecf06297a813283561bfe308a50cc3569fb20d912` (142 public tools; the site capture from the binary reports the same revision) |
| Catalog revision, platform | `sha256:257a3f615b68b4b889f182659dc3c9aa32540bfa2653d53ece099050884d0483` |
| Upgrade target | `rust-0.3.0-rc7-next`, `0.3.1-e2e`, the same commit, used only by the in-place upgrade journeys |

The dev VM was rebuilt from this set; its build refuses unless the installed
binary reports `0.3.0`. Go stays the published v0.2.4 release
(`baseline/source-lock.json`).

## 2. Gates and receipts

Every gate ran on the candidate under one run id, `m24-r1`
(`evidence/e2e/runs/m24-r1/`), with the m22 outcome rule, the m23 effect
rule and the m24 allowance rule on. `evidence/e2e/verify.py --gate` passes each gate. Every `tools/call`
and `tasks/get` step of every journey asserts its outcome. Every mutation a
journey expects to succeed is confirmed by a later observation, or opts out
with its reason. Every effects pattern matches a change of the journey's
own run and names no whole tree. `evidence/e2e/outcomes.py --check` flags no
step or pattern under the three rules, and `make e2e-test` runs it. Each journey names the spec requirements it proves in its
`results.json`.

| Gate | Journeys | Spec requirements passed | What it proves |
|---|---|---|---|
| e2e-0 | 12 | 12 | The harness can fail: undeclared effects, a broken pass condition, a schema violation, a tool error no assertion expects (`unchecked-outcome`, m22), a mutation nothing confirms or whose confirmation finds nothing (`unconfirmed-effect`, `pass-condition`, m23), and an allowance the run never uses or one that covers a whole tree (`unused-allowance`, `broad-allowance`, m24) are reported as failures, and the VM restores between journeys |
| e2e-1 | 8 | 20 | Install and first success: the system and user units, the npm launcher (packed locally), tutorial clients, auth and admission refusals with no durable writes, two agents and the writer lock, restart recovery, and migration from Go v0.2.4 with an in-place upgrade |
| e2e-2 | 5 | 30 | Typed domain work: host operations, Incus containers and VMs with ownership, cross-domain plans and cancellation, OCI build/push/run on K3s, PostgreSQL and SQLite data services |
| e2e-3a | 5 | 17 | A three-server HA K3s cluster built only through typed calls: workload, losing one server, quorum loss with fail-closed calls and recovery, and the optional cluster endpoint (Q19) |
| e2e-3 | 3 | 15 | Use cases: cluster discovery through the neutral name, the site deploy recipe, local model serving through the gateway (a test runtime) |
| e2e-4 | 2 | 10 | Cutover: Rust on Go v0.2.4's real state with work in flight, Rust's new work, rollback to Go on Rust's state; SIGTERM, SIGKILL and an in-place upgrade mid-work |
| e2e-6 | 10 | 14 | Host faults and pressure (m16, D31): a full state directory refuses effectful calls and recovers without a restart; a truncated store stops startup and the restored store starts intact; SIGKILL during a plan step and twelve SIGKILLs with tasks in flight leave a whole store; a hung or garbage-printing `incus` ends the call typed and the agent serves on; a clock stepped back an hour keeps tokens and reservations; admission refuses beyond the operation limit and below the floors; a 1 GiB declared body is answered 413 (D29), and so is one sent to the serving relay or to the k3s provider executable (D41, `e2e6.provider-body`); the 15-minute resource budget holds; a recipe step's child holds none of the agent's secrets (D40, `e2e6.child-env`); with a marker for each secret class, OAuth, an LLM relay, a host cluster agent and a public MCP connector leave no secret on any argv, in the journal, the store or a response, and every file holding one is 0600 (D46-D51, `e2e6.secret-sweep`); the connector call now gets past its origin probe (D52) |

Two earlier runs were retired with their cause: `m16-r1` (rc2) when fuzzing
found F5, and `m16-r2` (rc3) when CI's `cargo fmt` check failed and formatting
changed the candidate's source digest (`.parity/retired/m16-r1/`, `m16-r2/`).
In `m16-r3`, the first attempt of `e2e3a.lose-one` was killed by the round's
own 1780 s runner limit before writing any record, and the journey reran once
under the same run id (`.parity/retired/m16-r3-lose-one-timeout/`).
Run `m16-r3` named rc4 and is kept as its record; `m18-r1` named rc5 and is
kept as its record; `m19-r1` named rc6 and is kept as its record; `m21-r1`
named rc7 before the outcome rule and is kept as its record; `m22-r1`
named rc7 under the outcome rule and is kept as its record; `m23-r1`
named rc7 under both rules and is kept as its record; `m24-r1` replaces it
as the candidate's run. One journey start in `m19-r1`
(`e2e1.user-unit-npm`) was refused before it recorded anything, because the
dev VM's ports were still in TIME-WAIT after the previous journey's reboot; it
ran once the ports were free. In `m21-r1` every journey passed on its first
attempt. In `m22-r1` the first attempts of `e2e2.host-ops` and `e2e2.oci`
failed on downloads: the Ubuntu archive served the guest at 20-37 kB/s, and
the package steps' deadlines expired. They were retired with that cause
(`.parity/retired/m22-r1-apt-slow/`) and each reran once under the same run
id after the mirror recovered. In `m23-r1` and in `m24-r1` every journey
passed on its first attempt.

| Receipt | Where | What it proves |
|---|---|---|
| Contract suites | [`contract-suites.json`](../../../evidence/cutover/contract-suites.json), binary `f5b11757…` | D8 OAuth issuance 24/24, D10 standalone read-only gate 8/8, D11 rejected calls leave no audit writes 3/3, D12 refusal before an operation record 3/3, D13 evidence redaction 4/4 |
| Seam report | [`evidence/seam/m21`](../../../evidence/seam/m21) and its validation | D14: 133 compliant capabilities, 17 unported, no gap; the dispatch canary turns the verifier red |
| arm64 clean install | [`arm64/receipt.json`](../../../evidence/cutover/arm64/receipt.json), `host-agent-linux-arm64.gz` `b2396cc0…` | Debian 12 arm64 under emulation: checksum, install, launch, MCP `tools/list` and `get_host_info` with the exact agent id, SIGTERM stop, relaunch, stop, uninstall |
| Reproducible build | `make reproducible-build` (CI job `supply-chain`) | Two clean, offline, `--locked` builds of the agent and the four provider executables are byte-identical |
| Supply chain | `deny.toml`, `make supply-chain` (CI) | cargo-deny: advisories, licences, sources (crates.io only), duplicate versions; exceptions carry their reason (one accepted advisory, Q30) |
| Distribution | [`distribution.json`](../../../evidence/cutover/distribution.json) | Go's published-package read-only canary passes (1/1) on the locally packed 0.3.0 launcher starting the candidate; the site preview from the candidate's catalog equals Go's after the declared renames except in exactly the four tools D28 changed (below); Go's own `check-site` passes from Go v0.2.4 |

The distribution checks map onto tasks 5.1 and 5.2 as follows. All are local.

| Check | Evidence |
|---|---|
| Clean-machine install (x64) | `e2e1.system-unit`, `e2e1.user-unit-npm` (both in m24-r1) |
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
The inventory lists 25 entry points where untrusted bytes enter the agent;
each is covered by fuzz targets (17, cargo-fuzz on the stable toolchain) or
property tests (proptest), and a unit test fails if an entry loses its
coverage. CI fuzzes every target for 20 s; three local passes ran 60 s per
target, after a first was retired (results in the README, including the
targets that found nothing). m18 added three entry points (the serving
relay, a provider's `/mcp`, the child environment) and two properties, and
ran one more clean 60 s pass on rc5's code (`evidence/m18/fuzz.txt`); m19
ran another on rc6's code (`evidence/m19/fuzz.txt`), and m21 one on rc7's
(`evidence/m21/fuzz.txt`). m22 and m23 changed no Rust code.

| Id | Found by | What | State |
|---|---|---|---|
| F1 | property test | A resource id with Unicode whitespace changed identity on a round trip | fixed, D30 |
| F2 | fuzz `http_wire` | An `Authorization` header with a multi-byte character panicked `/oauth/token`'s connection | fixed |
| A1 | auth review | `/mcp` read the whole body before the 4 MiB check | fixed, D29 |
| F3 | e2e-6 draft | With the state directory full, an effectful call ran and its audit row was silently lost | fixed under D31: refused with `state_storage_full` below 1 MiB free |
| F4 | e2e-6 draft | `/health` ran `incus list` on the thread serving every connection; a hung `incus` stalled the agent | fixed |
| F5 | fuzz `json_document` | A `\u` escape with a sign was accepted (`from_str_radix`); three more decodes had the same defect | fixed |
| A2 | auth review | The model-serving relay forwards a request body without a bound | fixed in rc5, D41 by D43: 32 MiB, 413 beyond; `e2e6.pressure` |
| A4 | m15 review, measured on rc4 in m18 | Every tool the agent ran (incus, kubectl, git, curl, ollama, the `systemd-run` client) inherited its whole environment: `MCP_AUTH_TOKEN`, the OAuth clients and any provider credential | fixed in rc5, D40 by D43: an allowlist at all seven spawn sites; `e2e6.child-env` |
| A5 | m17, measured on rc4 in m18 | A provider executable's `/mcp` read the whole body before the 4 MiB check | fixed in rc5, D41 by D43; `e2e6.provider-body` |
| A3 | auth review | The bootstrap bearer's local-`Host` rule limits audience, not which peer may use it | as specified |
| S1 | m19 sweep, on rc5 | The public-MCP connector's tunnel token was on cloudflared's argv | fixed in rc6, D47 (`TUNNEL_TOKEN`); `e2e6.secret-sweep` |
| S2 | m19 review, unit test | The Cloudflare provider's host-placed connector had its run token in `ExecStart` | fixed in rc6, D48 (`--token-file`, 0600) |
| S3 | m19 review, unit test | The Tailscale join ran `tailscale up --authkey="$auth_key"` | fixed in rc6, D49 (`--auth-key=file:/dev/stdin`) |
| S4 | m19 sweep, on rc5 | `install_cluster_agent` carried the bridge token on the argv of `bash`, `sudo` and `incus exec`, sudo logged it, and the config was 0644 | fixed in rc6, D50 (stdin, 0600); `e2e6.secret-sweep` |
| S5 | m19 sweep, on rc5 | The store kept `ensure_local_llm_relay.upstreamToken` verbatim | fixed in rc6, D51 (`writeOnly`); `e2e6.secret-sweep` |
| S6 | m19, while fixing S2 | The Cloudflare HA connector in a guest had its run token in `ExecStart` of a 0644 unit | fixed in rc6, D48 |
| U1 | m21 upgrade chain, on rc6 | `ensure_public_mcp_tunnel` and `ensure_public_mcp_quick_tunnel` asked the agent's own token endpoint for a secretless `opute-mcp-host` token, which D8 refuses, so every call ended in HTTP 401 after starting the connector | fixed in rc7, D52 (an in-process token, as D8 requires); `e2e4.upgrade-chain` on `rust-0.4.0`, `e2e6.secret-sweep` |
| J1 | m22 outcome rule, on rc7 | `e2e6.store-damage` ran its agent as `opute`, which has no user manager on the VM, so its task failed (`Failed to connect to bus`) while the journey asserted only the status `completed` | journey defect, fixed: the agent runs as root and the task's output is asserted |
| E1-E4 | m22 outcome rule | Failures the journeys meant but did not assert: `tasks/get` of a never-persisted task (`task not found`, Q15); Go v0.2.4's first supervisor call before the user bus is up; the public-MCP connector stopping at its `.invalid` endpoint; `list_configmap_keys` and `list_secret_keys` not implemented (Q18) | expected refusals: each asserts `isError` and its text (Go's call opts out with its reason); [`evidence/m22`](../../../evidence/m22/README.md) |
| J2 | m23 effect rule, on rc7 | `e2e2.data` checked SQLite removal under `~/.opute/databases`, a path the agent never uses (it keeps them under the instance's `databases` root), so the check could never fail and cleanup left the databases | journey defect, fixed: the check, the plans' read-backs and every cleanup use the agent's root |
| J3 | m23 effect rule, its own draft on rc7 | The new read-back of `e2e1.restart`'s resubmission assumed the run stayed `unknown`; the resubmission resumes the same durable run (`running`), and Q15 requires only that it is admitted | journey defect, fixed: same run id, `running` or `completed`; [`evidence/m23`](../../../evidence/m23/README.md) |
| W1 | m24 allowance rule | `e2e2.host-ops` allowed `~/evil`, the file a path-traversal escape would write, so an escape would have passed the effects check | journey defect, fixed: removed |
| W2-W5 | m24 allowance rule | Patterns that could not match (`packages/*`, `users/*`, `groups/*`, kinds the state never records), units stopped before the after-state, patterns covering a whole tree (`files//etc*`, `incus/*`, `system/*`, `files//etc/systemd/system/*`), and `persist` patterns no run left residue for | journey defects, fixed: each pattern names its run's change; [`evidence/m24`](../../../evidence/m24/README.md) |
| O1 | e2e-6 budget | Audit rows are never pruned: about 3.5 KB per call, 78 MB in 15 minutes of heavy load | open in 0.3.0: D42 sets retention for 0.4.0 only (D43) |

Each m18 fix has a test that failed on rc4's code
([`evidence/m18`](../../../evidence/m18/README.md)); each m19 fix has one that
failed on rc5's ([`evidence/m19`](../../../evidence/m19/README.md)); the m21
fix has one that failed on rc6's ([`evidence/m21`](../../../evidence/m21/README.md)).

The full auth walk is [`auth-review.md`](../../../evidence/hardening/auth-review.md).

Resource budget (`resource-budget.json`), on the candidate in `e2e6.budget`
(run `m24-r1`): 25,846 calls in 15 minutes with no error; after a two-minute
warm-up RSS grew 15.6 MB (budget 48 MiB), open FDs and threads did not grow,
and the store grew 3,421 bytes per call (budget 8 KiB; the first budget's
absolute 64 MiB store limit had no measured basis and was replaced; audit
rows are still never pruned in 0.3.0, O1). Peak RSS 95 MB (budget 256 MiB).
The D29 check: a 1 GiB declared body was answered 413 at once, with RSS 43 MB
afterwards. The D41 check: a 1 GiB declared body to the serving relay was
answered 413 in 0.03 s, with RSS 61 MB afterwards (53 MB before); one to the
k3s provider executable's `/mcp` was answered 413 at once, with its RSS
2.6 MB before and 2.8 MB after.

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
| D40 (by D43) | A process the agent starts gets an allowlisted environment, never a secret-named variable; Go passed its whole environment | Host Agent owner, delegated 2026-11-11 |
| D41 (by D43) | The serving relay reads at most 32 MiB and a provider's `/mcp` at most 4 MiB of a request body, 413 beyond | Host Agent owner, delegated 2026-11-11 |
| D46 | A secret never travels on a command line; a file holding one is 0600 and the agent's user's; the journal, the store and responses carry it only redacted | Host Agent owner, delegated 2026-11-11 |
| D47 | The public-MCP connector gets its tunnel token from `TUNNEL_TOKEN`, not `--token` | Host Agent owner, delegated 2026-11-11 |
| D48 | Cloudflare host-placed and HA connectors read the run token with `--token-file` from a 0600 file | Host Agent owner, delegated 2026-11-11 |
| D49 | The Tailscale join reads its auth key from stdin (`--auth-key=file:/dev/stdin`) | Host Agent owner, delegated 2026-11-11 |
| D50 | The cluster agent's config goes on stdin and is written 0600; Go embedded it base64-encoded in the install script | Host Agent owner, delegated 2026-11-11 |
| D51 | `ensure_local_llm_relay.upstreamToken` is published `writeOnly` | Host Agent owner, delegated 2026-11-11 |
| D52 | The public-MCP connector probes the agent's own resources with an in-process token; Go asked its token endpoint without a secret | Host Agent owner, delegated 2026-11-11 |

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
| O1 audit retention | open observation | Host Agent owner | D42 decides it for 0.4.0; D43 keeps 0.3.0 without a new setting. Not a cutover blocker by the spec. A2, A4 and A5 are fixed in rc5, S1 to S6 in rc6, U1 in rc7 (section 2a) |

`make cutover-readiness` lists exactly e2e-5, WSL2, Q4 and Q18 as not passed.

## 5. Cutover and rollback runbook

For the deployment owner, after sign-off. Each step names the journey that
rehearsed it on the dev VM with the candidate (run `m24-r1`). Production
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

## 7. Rust 0.4.0 candidate (goals m15, m17, m20, m19, m21, m22, m23 and m24)

Sections 1 to 6 are the 0.3.0 candidate, and its sign-off stands alone.
This section is the next candidate, from the integration branch
`rust-0.4.0`, which reaches main only after the 0.3.0 sign-off. Nothing
here was tagged or published. On `rust-0.4.0`, `evidence/cutover/candidate.json`
names this set and `make cutover-readiness` checks it.

### 7.1 What changed from 0.3.0

Every tool the agent publishes now either works or is withdrawn by
decision. The 0.3.0 seam report listed 17 unported capabilities; the 0.4.0
report (`evidence/seam/m17`) lists 144 compliant, 2 unported and 4
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

The catalog revision for standalone was re-pinned for D37 and D39, and
again for D51 (m19), to
`sha256:6ceca36091c4fc214fac50a643f9dfad87cb4163fb6ebe8b1913a57286e0178c`.

Goal m17 closed the open observations in this candidate. Sections 2a and 4
are the 0.3.0 candidate, rc5: goal m18 fixed A2, A4 and A5 there too (D43),
and O1 stays open in 0.3.0. The decisions were
delegated by the Host Agent owner on 2026-11-11.

| Id | What | State in 0.4.0 |
|---|---|---|
| A4 (found in m15) | Every process the agent started inherited its whole environment, bearer and provider credentials included | Closed by D40, [child-process-environment](../child-process-environment/proposal.md): an allowlist at all eight spawn sites, never a secret-named variable |
| A2 (m16) | The serving relay read a request body whole | Closed by D41, [bound-relay-request-body](../bound-relay-request-body/proposal.md): 32 MiB, 413 beyond |
| A5 (found in m17) | The provider executables' `/mcp` read a body whole before the 4 MiB check | Closed by D41: the SDK's 4 MiB, 413 beyond |
| O1 (m16) | Audit rows were never pruned | Closed by D42, [audit-retention](../audit-retention/proposal.md): 30 days and 50,000 rows by default (`OPUTE_AUDIT_RETENTION_DAYS`, `OPUTE_AUDIT_RETENTION_ROWS`); finished operations follow after their task's one-hour TTL |

Each fix has a test that failed on rc1's code
([`evidence/m17`](../../../evidence/m17/README.md)).

Goal m19 swept every secret's placement (D46,
[secret-placement](../secret-placement/proposal.md)) and found the same six
exposures in 0.4.0 as in 0.3.0 (section 2a); `e2e6.secret-sweep` measured
S1, S4 and S5 on rc3 (run `m19-before-040`). All six are closed in rc4 by the
same fixes as in 0.3.0 rc6:

| Id | Exposure | State in 0.4.0 |
|---|---|---|
| S1 | The public site's cloudflared run token on argv | Closed by D47, [public-tunnel-token-environment](../public-tunnel-token-environment/proposal.md) |
| S2, S6 | The host and HA cloudflared connectors' run token in `ExecStart` | Closed by D48, [cloudflare-connector-token-file](../cloudflare-connector-token-file/proposal.md): a 0600 token file and `--token-file` |
| S3 | The Tailscale auth key on `tailscale up`'s argv | Closed by D49, [tailscale-auth-key-file](../tailscale-auth-key-file/proposal.md): `--auth-key=file:/dev/stdin` |
| S4 | The cluster agent's bridge token on argv inside its install script, in sudo's journal entry and in a 0644 config | Closed by D50, [cluster-agent-config-stdin](../cluster-agent-config-stdin/proposal.md): the config travels on stdin into a 0600 file |
| S5 | `ensure_local_llm_relay`'s `upstreamToken` stored verbatim | Closed by D51, [write-only-relay-upstream-token](../write-only-relay-upstream-token/proposal.md) |

Each fix has a test that fails on rc3's code
([`evidence/m19/before-040.log`](../../../evidence/m19/before-040.log)).

Goal m22 put the outcome rule (section 1's m22 note) on this line too. Its
own journeys had 78 flagged steps before the round; the findings in 0.4.0
match 0.3.0's ([`evidence/m22`](../../../evidence/m22/README.md)), and none
is a product defect:

| Id | Journeys | Finding | State in 0.4.0 |
|---|---|---|---|
| J1 | `e2e6.store-damage` | Its task failed on the user bus, unasserted | journey defect, fixed as in 0.3.0 |
| E1, E2 | `e2e1.restart`, `e2e4.cutover` | A never-persisted task's `task not found`; Go v0.2.4's first supervisor call | expected; asserted, or opted out with its reason |
| E3 | `e2e6.secret-sweep`, and `e2e4.upgrade-chain`'s three tunnel calls and three re-reads | The connector stops at its `.invalid` public endpoint, past its origin | expected refusal; `isError: true` and the text asserted |
| — | `e2e3.discovery`, `e2e3a.workload`, `e2e3a.lose-one`, `e2e6.child-env`, `e2e7.public-site`, `neutral.reserved` | Steps that returned success with no assertion on it (the key tools, the env edits, the console input, the site's host prerequisites, the provider installs) | each now asserts its result |

Goal m23 put the effect rule (section 1's m23 note) on this line too. Its
journeys had 279 unconfirmed mutation steps before the round. All are now
confirmed by a later observation, or opt out with their reason
([`evidence/m23`](../../../evidence/m23/README.md)). The findings match
0.3.0's, and none is a product defect:

| Id | Journeys | Finding | State in 0.4.0 |
|---|---|---|---|
| J2 | `e2e2.data` | SQLite removal was checked at a path the agent never uses | journey defect, fixed as in 0.3.0 |
| J3 | `e2e1.restart` | m23's own read-back assumed the resubmitted run stayed `unknown`; it resumes | journey defect, fixed as in 0.3.0 |
| — | `e2e2.incus`, `e2e3.serving`, `e2e3a.workload`, `e2e3a.lose-one`, `e2e4.upgrade-chain`, `e2e6.child-env`, `e2e7.public-site`, `neutral.*` | Mutations only 0.4.0 runs: the console, the NIC, the model removal, the ConfigMap and env edits, the release path's writes, plans and relay, the providers' units, the recipe files, the site | each confirmed: a guest read, a file digest, a listening port, `list_configmap_keys`, `inspect_workload` or the fetched page; console input and resize opt out (only the console session sees them) |

Goal m24 put the allowance rule (section 1's m24 note) on this line too.
Before the round, 138 of 386 allow patterns and 17 of 27 persist patterns
matched nothing in their committed run, and 61 covered a whole tree. Each
now names what its run changes ([`evidence/m24`](../../../evidence/m24/README.md)).
The findings match 0.3.0's, and none is a product defect:

| Id | Journeys | Finding | State in 0.4.0 |
|---|---|---|---|
| W1-W5 | as in 0.3.0 | An allowed traversal-escape target, patterns for kinds the state never records, stale units, whole-tree patterns, persist without residue | journey defects, fixed as in 0.3.0 |
| — | `e2e4.upgrade-chain`, `e2e6.child-env`, `e2e7.public-site`, `neutral.*` | Patterns only 0.4.0 runs had: `incus/*`, `system/units/*`, `user/*`, `files//etc/systemd/system/*`, every agent's unit by wildcard, the 0.3.x release in journeys that never unpack it, and persist patterns for the site's build context and instance state, which cleanup removes | journey defects, fixed: the console's VM and its image, the chain's drop-in, each agent's and provider's unit by name; no persist |

### 7.2 The candidate

| Field | Value |
|---|---|
| Name | `rust-0.4.0-rc4`, version `0.4.0`. It replaces `rust-0.4.0-rc3` (m20, run `m20-r1`), which D47 to D51 (m19) superseded |
| Commit | `2d2f51d20509ae49205be8f2d8843b15e783bb42` (branch `claude/m19-secret-sweep-040`) |
| `SHA256SUMS` | `sha256:fc41085ba5a35e3ba453aed8680c7671563e4e949ef801f53aefb3ec7d76814e` |
| `host-agent-linux-x64` | `8fcb0352a1f01f3072a5a1ab7bf005528f618c14936e30665a5ad5e6fbd5a78b` |
| `host-agent-linux-arm64.gz` | `2505b3e6247699e66fb21b61bdc80e427a8234a55ac5f17a75d5750dd2bb8e60` |
| Upgrade target | `rust-0.4.0-rc4-next`, `0.4.1-e2e`, the same commit |
| Rollback target | `rust-0.3.0-rc7` (section 1), used by `neutral.rollback` and `e2e4.upgrade-chain` (m21; it was `rust-0.3.0-rc4`) |

Before the build: `cargo fmt` and clippy were clean, a 60 s pass over all
18 fuzz targets found nothing ([`evidence/m19/fuzz-040.txt`](../../../evidence/m19/fuzz-040.txt)),
and the stale grep found no earlier candidate named as current. The 26 hardening entry points include the
console-output reader (m15) and, from m17, the serving relay, the provider
`/mcp` and the child environment, each with a property test.

### 7.3 Gates and receipts

Every gate ran on rc4 under run `m19-r2`, and `evidence/e2e/verify.py --gate`
passes each. Goal m21 changed no 0.4.0 code. It added `e2e4.upgrade-chain`,
pinned the rollback journeys to `rust-0.3.0-rc7` and taught the sweep's scan
which owners a private file may have, so it reran gates e2e-4, e2e-neutral
and e2e-6 on rc4 under run `m21-r2`. Every journey there passed on its first
attempt. Goal m22 changed no 0.4.0 code either; with the outcome rule on,
every gate reran on rc4 under run `m22-r2`. Goal m23 changed no 0.4.0 code.
With the outcome and effect rules on, every gate reran on rc4 under run
`m23-r2`. Goal m24 changed no 0.4.0 code. With the three rules on, every
gate reran on rc4 under run `m24-r2`, which is now the run of every gate. `evidence/e2e/verify.py --gate`
passes each.

| Gate | Journeys | Spec requirements passed | New in 0.4.0 |
|---|---|---|---|
| e2e-0 | 12 | 12 | `selfcheck.unchecked-outcome` (m22); `selfcheck.unconfirmed-effect` and `selfcheck.empty-confirmation` (m23); `selfcheck.unused-allowance` and `selfcheck.broad-allowance` (m24) |
| e2e-1 | 8 | 35 | Install on the proposed unit and launcher (below); upgrade to `0.4.1-e2e` |
| e2e-2 | 5 | 38 | `e2e2.incus`: the console journey (open, read the login prompt, resize, detach, no `incus console` left) and the NIC journey (attach, attach again, remove, remove again) |
| e2e-3a | 5 | 33 | `e2e3a.workload`: cluster inspection on the HA cluster through `list_secret_keys`, `list_configmap_keys` and `inspect_workload`; the Q18 workaround is gone |
| e2e-3 | 3 | 19 | `e2e3.serving` calls both LLM tools; `e2e3.discovery` reads K3s's own keys |
| e2e-4 | 3 | 16 | `e2e4.upgrade-chain` (m21): the release path on one host, on the production unit. Go v0.2.4 on Go's unit leaves a run in flight. `rust-0.3.0-rc7` serves that state on the same unit, and installs an LLM relay and a user-scoped public MCP connector with marked tokens. rc4 takes over on the proposed 0.4.0 unit. rc7 is then rolled back onto that unit. At each step, counts and ids reconcile per table, every run, task and operation reads back with the status last reported (Go's in-flight run as `cancelled`), mutations are admitted from the unit, each connector call gets past its origin probe, and no token is in the journal, the store, a non-private file or another process's environment (D52) |
| e2e-6 | 9 | 21 | `e2e6.secret-sweep` (m19): with markers planted for an OAuth grant, an LLM relay, a host cluster agent install and a public MCP connector, no marker is on any process's argv (78 execs traced host-wide), in the journal, in the store or in a response, and every file that holds one is 0600; the environ of the agent's own unit holds its own credentials only. `e2e6.child-env`: `/proc` shows that neither a console's `incus` process nor a recipe step's `systemd-run` client holds the agent's bearer, a provider credential or an `OPUTE_` setting (D40). `e2e6.pressure`: a 1 GiB body to the serving relay is answered 413 in 0.03 s, with RSS at 58 MB (D41). `e2e6.budget` (`m23-r2`): with a 5,000-row bound, 28,003 calls leave exactly 5,000 audit rows one prune interval later, and the store grows 771 bytes per call against 3,466 in 0.3.0 (D42) |
| e2e-neutral | 3 | 13 | Rerun after PR #43's merge; rollback from 0.4.0 to 0.3.0 rc7 |
| e2e-7 | 1 | 5 | `e2e7.public-site` (m20): one `run_host_local_recipe` run brings a K3s Service up at `e2e-m24-r2.opute.io`, which the host fetches over verified TLS; a second run skips every node; a deleted pod is replaced; neither the Cloudflare token nor the tunnel's run token is in any file, journal line or run record; one teardown run removes the guest, the CNAME and the tunnel, and a second skips (D44, D45) |

Retired, with cause (`.parity/retired/`):
- `m15-r1-stale-journeys`: the first attempts of `e2e1.migrate-upgrade`
  and `e2e4.restart` expected the version `0.3.1-e2e`, and `e2e3.discovery`
  expected `not_implemented` from the two key tools. These were journey
  expectations left from 0.3.0, not product defects.
- `m15-r1-neutral-reserved-provider`: the first attempt of `neutral.reserved`
  passed on another build's Cloudflare provider.

Each of these journeys reran once under the same run, on the same build.
Those were rc1's. On rc2:
- `m17-r1-host-restart`: the host machine restarted three journeys into the
  round. No verdict; the dev VM was rebuilt from rc2 and the whole round
  reran as `m17-r2`.
- `m17-r2-quorum-removal-dns`: the first attempt of `e2e3a.quorum-removal`
  failed when the third server could not resolve `github.com` to download
  K3s. Environmental; the journey reran once under `m17-r2` and passed.

On rc3 (`m20-r1`), every journey of the eight earlier gates passed on its first
attempt. One attempt was retired:
- `m20-r1-e2e7-first`: the first attempt of `e2e7.public-site` brought the
  site up and served it, then failed on a recipe defect and three journey
  defects, none in the binary. The teardown recipe removed the tunnel before
  the guest; `remove-host-tunnel` stops only a host-placed connector, so the
  in-cluster connector still held connections and Cloudflare refused the
  delete (code 1022). The recipe now removes the guest first and retries the
  tunnel node; on the rerun it succeeded on attempt 4 of 6. The journey
  deleted the pod without the client capabilities the tool requires, used an
  incus project the agent's guests are not in, and checked a provider tool
  against a catalog read before the providers were installed.

On rc4 (`m19-r2`), one attempt was retired:
- `m19-r2-e2e3a-workload-dns`: the first attempt of `e2e3a.workload` failed
  when a guest could not resolve `get.k3s.io`. Environmental; the journey
  reran once under `m19-r2` and passed in 228 s.

On rc4 under the outcome rule (`m22-r2`), one attempt was retired:
- `m22-r2-endpoint-dns`: the first attempt of `e2e3a.endpoint` failed when
  the K3s provider could not resolve `github.com` for the release checksums,
  and every later step depended on that cluster. Environmental; the journey
  reran once under `m22-r2`.

On rc4 under both rules (`m23-r2`), every journey passed on its first attempt.

| Receipt | What it proves |
|---|---|
| Contract suites | Binary `8fcb0352…`. D8 OAuth issuance passes 23/23, D10 7/7, D11 3/3, D12 3/3 and D13 4/4. PR #43 (platform-neutral) removed two 0.3.0 scenarios. `credentials.operator-secret` went because OAuth machine clients now come from `OPUTE_OAUTH_CLIENTS`. `gate.platform-unaffected` went because there is one serving profile and no mode |
| Seam `m19` | 144 compliant, 2 unported, 4 unpublished, no gap, the red canary, and a rerun that reproduces the report byte for byte |
| arm64 clean install | `host-agent-linux-arm64.gz` `2505b3e6…`, 89 s |
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

Measured by `e2e4.upgrade-chain` (m21) on one host:
1. Upgrade from 0.3.0 to 0.4.0: stop the unit, install the proposed unit
   and the 0.4.0 binary on the same instance root, env file and drop-ins,
   and start it. 0.4.0 reconciles 0.3.0's state per table and reads back
   every run, task and operation, including the run Go stopped in flight
   (`cancelled`).
2. Rollback from 0.4.0 to 0.3.0: stop the unit and put the 0.3.0 binary
   back, keeping the proposed unit. 0.3.0 reads 0.4.0's state the same way,
   0.4.0-era work included, and admits mutations from that unit's 0.3.0
   settings.
3. Both directions keep the relay and connector files private (0600, the
   agent's user's). A connector reconciled by one line is reconciled again
   by the other with no second unit. Each line needs its own OAuth settings
   for that: `OPUTE_HOST_OAUTH_CLIENT_SECRET` for 0.3.0 and
   `OPUTE_OAUTH_CLIENTS` for 0.4.0.

### 7.5 Not passed

`make cutover-readiness` on `rust-0.4.0` lists e2e-5, WSL2 and Q4, the same
owners and unblockers as section 4. Q18 is answered by D32 and passes. No
observation from the hardening review is open in 0.4.0.
