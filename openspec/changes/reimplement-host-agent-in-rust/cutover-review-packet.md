# Cutover review packet: Rust 0.3.0 release candidate

For task 6.3: the Host Agent, Platform, release and deployment owners review
one release candidate and the evidence that names it. This packet selects
nothing and authorizes nothing. Go v0.2.4 stays the production owner until
the owners sign below and the responsible owner authorizes cutover through
its normal rollout path (spec, *Cutover requires specification-based
evidence*). The owners signed off on rc7 on 2026-11-21. D53 reopens
review under task 6.3 for rc8 (section 6).

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

m25 extends the journey observer to additional host files and runtime state.
It found that provider executables listened beyond loopback (P1). D53
restricts the binding to `127.0.0.1`. The successor candidate is
`rust-0.3.0-rc8`. The expanded journey evidence verifies under run `m25-r1`.

## 1. The candidate

| Field | Value |
|---|---|
| Name | `rust-0.3.0-rc8` ([`evidence/cutover/candidate.json`](../../../evidence/cutover/candidate.json)) |
| Version | `0.3.0` (the binary's `--version`; D28 makes 0.3.0 the first Rust package) |
| Commit | `a8c018197e6ba0136c02274d9d7a78bad2973655` (branch `claude/m25-observer-coverage`) |
| Built | locally with `tools/release/build-artifacts.sh`, `VERSION=0.3.0`; no tag, no publish. The set carries a CycloneDX SBOM, `sbom.cdx.json`, with its own checksum |
| `SHA256SUMS` | `sha256:e87d480ed303d2dc859fd859c2cf54965076aa12031801fb3ef7dafd4904aa87` |
| `host-agent-linux-x64` | `0cd61080aad40c6e40730f9a96d49adddbbb43aa06ae390c35957e848089fc60` |
| `host-agent-linux-x64.gz` | `f8fe9ea72594cd17c4d51bdcbb1dc653c2fe6df00a07ad535a2d56f583628817` |
| `host-agent-linux-arm64.gz` | `c9f77a7721a52fe6ccee21cfc5701eb0ec264cf39a2715c4cfc1d06e638cdb03` |
| `opute-provider-k3s-linux-x64` | `5726d74524c3b68948feb78a045ee1ed2191971a4e94f7ce7a725a2f479429bc` |
| `opute-provider-cloudflare-linux-x64` | `9f8224dfd60932b0dfcfd1847ee810e3efdc9ec90f33e331fe8847bc012d875d` |
| `opute-provider-tailscale-linux-x64` | `975b3fd51f223f0c7bc5e6ccda2ada8775d12fce67b5bedd1a268c3e272238c4` |
| Catalog revision, standalone | `sha256:9aba4c895695d25c6b8f91aecf06297a813283561bfe308a50cc3569fb20d912` (142 public tools; the site capture from the binary reports the same revision) |
| Catalog revision, platform | `sha256:257a3f615b68b4b889f182659dc3c9aa32540bfa2653d53ece099050884d0483` |
| Upgrade target | `rust-0.3.0-rc8-next`, `0.3.1-e2e`, the same commit, used only by the in-place upgrade journeys |

The dev VM build verifies that the installed binary reports `0.3.0`. Go stays the published v0.2.4 release
(`baseline/source-lock.json`).

## 2. Gates and receipts

All seven runnable gates verified on rc8 under `m25-r1` with the m22
outcome rule, the m23 effect confirmation rule, the m24 allowance rule
and the m25 host observer. The verifier matched all 59 receipts,
including the exact effects paths of the 14 new probes.

| Gate | Journeys | Spec requirements passed | What it proves |
|---|---|---|---|
| e2e-0 | 26 | 25 | The harness can fail: undeclared effects, a broken pass condition, a schema violation, a tool error no assertion expects (`unchecked-outcome`, m22), a mutation nothing confirms or whose confirmation finds nothing (`unconfirmed-effect`, `pass-condition`, m23), and an allowance the run never uses or one that covers a whole tree (`unused-allowance`, `broad-allowance`, m24) are reported as failures, and the VM restores between journeys Fourteen probes identify their exact planted filesystem and runtime effects. |
| e2e-1 | 8 | 34 | Install and first success: the system and user units, the npm launcher (packed locally), tutorial clients, auth and admission refusals with no durable writes, two agents and the writer lock, restart recovery, and migration from Go v0.2.4 with an in-place upgrade |
| e2e-2 | 5 | 32 | Typed domain work: host operations, Incus containers and VMs with ownership, cross-domain plans and cancellation, OCI build/push/run on K3s, PostgreSQL and SQLite data services |
| e2e-3a | 5 | 29 | A three-server HA K3s cluster built only through typed calls: workload, losing one server, quorum loss with fail-closed calls and recovery, and the optional cluster endpoint (Q19) |
| e2e-3 | 3 | 17 | Use cases: cluster discovery through the neutral name, the site deploy recipe, local model serving through the gateway (a test runtime) |
| e2e-4 | 2 | 11 | Cutover: Rust on Go v0.2.4's real state with work in flight, Rust's new work, rollback to Go on Rust's state; SIGTERM, SIGKILL and an in-place upgrade mid-work |
| e2e-6 | 10 | 21 | Host faults and pressure (m16, D31): a full state directory refuses effectful calls and recovers without a restart; a truncated store stops startup and the restored store starts intact; SIGKILL during a plan step and twelve SIGKILLs with tasks in flight leave a whole store; a hung or garbage-printing `incus` ends the call typed and the agent serves on; a clock stepped back an hour keeps tokens and reservations; admission refuses beyond the operation limit and below the floors; a 1 GiB declared body is answered 413 (D29), and so is one sent to the serving relay or to the k3s provider executable (D41, `e2e6.provider-body`); the 15-minute resource budget holds; a recipe step's child holds none of the agent's secrets (D40, `e2e6.child-env`); with a marker for each secret class, OAuth, an LLM relay, a host cluster agent and a public MCP connector leave no secret on any argv, in the journal, the store or a response, and every file holding one is 0600 (D46-D51, `e2e6.secret-sweep`); the connector call now gets past its origin probe (D52) |

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
as the rc7 run. One journey start in `m19-r1`
(`e2e1.user-unit-npm`) was refused before it recorded anything, because the
dev VM's ports were still in TIME-WAIT after the previous journey's reboot; it
ran once the ports were free. In `m21-r1` every journey passed on its first
attempt. In `m22-r1` the first attempts of `e2e2.host-ops` and `e2e2.oci`
failed on downloads: the Ubuntu archive served the guest at 20-37 kB/s, and
the package steps' deadlines expired. They were retired with that cause
(`.parity/retired/m22-r1-apt-slow/`) and each reran once under the same run
id after the mirror recovered. In `m23-r1` and in `m24-r1` every journey
passed on its first attempt. In `m25-r1`, the first data journey exposed
an Incus loopback proxy that remains available until guest cleanup.
The attempt was archived at `.parity/retired/m25-r1-e2e2.data-incus-proxy`.
The journey verified the socket against its Incus device and passed on
its single rerun.

| Receipt | Where | What it proves |
|---|---|---|
| Contract suites | [`contract-suites.json`](../../../evidence/cutover/contract-suites.json), binary `0cd61080…` | D8 OAuth issuance 24/24, D10 standalone read-only gate 8/8, D11 rejected calls leave no audit writes 3/3, D12 refusal before an operation record 3/3, D13 evidence redaction 4/4 |
| Seam report | [`evidence/seam/m25`](../../../evidence/seam/m25) and its validation | D14: 133 compliant capabilities, 17 unported, no gap; the dispatch canary turns the verifier red |
| arm64 clean install | [`arm64/receipt.json`](../../../evidence/cutover/arm64/receipt.json), `host-agent-linux-arm64.gz` `c9f77a77…` | Debian 12 arm64 under emulation: checksum, install, launch, MCP `tools/list` and `get_host_info` with the exact agent id, SIGTERM stop, relaunch, stop, uninstall |
| Reproducible build | `make reproducible-build` (CI job `supply-chain`) | Two clean, offline, `--locked` builds of the agent and the four provider executables are byte-identical |
| Supply chain | `deny.toml`, `make supply-chain` (CI) | cargo-deny: advisories, licences, sources (crates.io only), duplicate versions; exceptions carry their reason (one accepted advisory, Q30) |
| Distribution | [`distribution.json`](../../../evidence/cutover/distribution.json) | Go's published-package read-only canary passes (1/1) on the locally packed 0.3.0 launcher starting the candidate; the site preview from the candidate's catalog equals Go's after the declared renames except in exactly the four tools D28 changed (below); Go's own `check-site` passes from Go v0.2.4 |

The distribution checks map onto tasks 5.1 and 5.2 as follows. All are local.

| Check | Evidence |
|---|---|
| Clean-machine install (x64) | `e2e1.system-unit`, `e2e1.user-unit-npm` (both in m25-r1) |
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
| P1 | m25 socket observation on rc7 | Provider executables listened on all host interfaces | D53 binds providers to IPv4 loopback. See [m25 evidence](../../../evidence/m25/README.md). |
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
| D53 | Provider executables listen on IPv4 loopback at their configured port | Host Agent owner, 2026-11-21 |

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

The 2026-11-21 approval covers rc7 on `m24-r1`. The provider listener
change in D53 reopens review for the successor candidate. The owner rows
below preserve the historical decision.


The owner, who holds all four roles, signed off in the session on
2026-11-21 ("I'm signing off on your recommendations") and chose to record
it in every row. The approval covers rc7 and its evidence on run `m24-r1`. It does not tag, publish or select a production host:
cutover through the normal rollout path is a separate, explicit step, and Go
stays the production owner until then (task 6.3).

| Owner | Name | Decision | Date | Notes |
|---|---|---|---|---|
| Host Agent | Houman Kamali (wunderous) | Approved: `rust-0.3.0-rc7` | 2026-11-21 | Q18 stays deferred as a new capability (D28); O1 stays open in 0.3.0 (D43) |
| Opute Platform | Houman Kamali (wunderous) | Approved: `rust-0.3.0-rc7` | 2026-11-21 | e2e-5 and Q4 stay open (section 4) |
| Release | Houman Kamali (wunderous) | Approved: `rust-0.3.0-rc7` | 2026-11-21 | WSL2 stays blocked; publishing, npm and the site's catalog claims (5.2) stay separate steps (D28) |
| Deployment | Houman Kamali (wunderous) | Approved: `rust-0.3.0-rc7` | 2026-11-21 | Cutover follows the section 5 runbook when the owner starts it; rollback window 14 days |
