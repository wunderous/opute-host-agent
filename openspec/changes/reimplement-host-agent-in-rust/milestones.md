# Milestones and end-to-end validation plan

This document breaks the port into ordered milestones and defines the
end-to-end (E2E) evidence each one must produce. It refines
[tasks.md](tasks.md); the task IDs remain the gate of record, and every
milestone below names the task IDs it closes. Like the rest of this change, it
is a plan. It does not claim that any milestone has started or passed.

Facts about the Go baseline cited here were read from the pinned tree
`ace7013df17528fee1bed13a1d70a132d6c5eb9b`. Milestone 0 must re-verify them
into the machine-readable inventory before anything depends on them.

## 1. Why the plan is shaped this way

Two properties shape the sequence:

1. **A differential harness comes first.** "Parity" is only checkable if the
   same scenario runs against Go and Rust and a machine compares the results.
   If each milestone wrote its own ad-hoc tests, we would be comparing Rust
   against our memory of Go. Milestone 0 therefore builds one harness, proves
   it can detect differences, and every later milestone only adds scenarios.
2. **Order follows dependency and blast radius.** Read-only wire behavior is
   cheap to compare and has no side effects. Mutations, plans, providers, and
   durable state build on it and carry real risk. Each milestone ratchets:
   earlier scenarios keep running, so a later change cannot silently regress
   an earlier boundary.

```text
 M0  harness + inventory + verifier   (nothing runs in Rust yet)
  │
 M1  skeleton, CLI, config, lifecycle ─────────────┐
  │                                                │
 M2  HTTP transport, auth, MCP wire                │  read-only,
  │                                                │  shim fixtures only
 M3  catalog + read-only host tools ───────────────┘
  │
 M4  admission, resources, tasks, cancellation      rejects before effects
  │
 M5  durable state + redaction                      ◄── needed by M6/M7
  │
  ├────────────► M6  recipes + one plan executor
  │                    │
  └────────────► M7  provider lifecycle (hosts unchanged Go providers)
                       │
 M8  mutating domain operations (8a…8f, one domain at a time)
  │
 M9  provider executables in Rust (optional; per-provider decision)
  │
 M10 packaging, installer, npm launcher, site
  │
 M11 real clients + Platform integration
  │
 M12 state conversion, soak, cutover rehearsal
  │
 M13 owner cutover decision  ──►  (later, separate) Go retirement
```

M6 and M7 can proceed in parallel once M5 has passed. M8 domains can be split
among contributors once M4–M7 have passed.

## 2. The E2E harness: one design, used by every milestone

### 2.1 Twin runs against isolated, identical fixtures

```text
                    scenario (YAML/JSON, versioned)
                               │
               ┌───────────────┴───────────────┐
               ▼                               ▼
   ┌───────────────────────┐       ┌───────────────────────┐
   │ Go reference binary    │       │ Rust candidate binary  │
   │ built from pinned SHA  │       │ built from this repo   │
   │ ID: parity-go-<run>    │       │ ID: parity-rs-<run>    │
   │ port, token, state dir │       │ port, token, state dir │
   │ all distinct           │       │ all distinct           │
   └──────────┬────────────┘       └──────────┬────────────┘
              │ PATH / *_BINARY_PATH           │
              ▼                                ▼
   ┌───────────────────────┐       ┌───────────────────────┐
   │ recording shims        │       │ recording shims        │
   │ incus systemctl helm … │       │ (identical scripts)    │
   │ → trace.jsonl          │       │ → trace.jsonl          │
   └──────────┬────────────┘       └──────────┬────────────┘
              ▼                                ▼
      observations: exit code, stdout/stderr, HTTP status/headers,
      JSON-RPC bodies, task event streams, shim traces, SQLite rows,
      files + modes, listening sockets, log lines
               └───────────────┬───────────────┘
                               ▼
               canonicalize → mask declared volatile fields
                               ▼
                 diff → parity item (pass | fail | unverified)
                               ▼
                 evidence bundle → parity manifest → verifier
```

The rules come from the design and are not optional:

- **Same scenario, separate worlds.** Go and Rust never share a state
  directory, port, identity, credential, or mutable external resource. Each
  side receives a fresh copy of the same fixture.
- **Canonical comparison.** JSON is compared structurally (key order does not
  matter; array order matters unless the contract says it is a set). Volatile
  values such as timestamps, generated IDs, ports, and durations are masked
  only through **typed, per-field declarations** in the manifest (for example
  `result.operationId: uuid-v4`). An unmasked difference fails. Regex masks
  over whole payloads are not allowed.
- **Masked values keep their shape.** A masked field must still validate
  against its declared type. Masking a timestamp must not hide a missing
  timestamp.
- **Each run has a provenance record:** Go SHA and tree, Rust commit, binary
  hashes, fixture hashes, scenario hash, mode, provider set, and catalog
  revision. The verifier rejects evidence whose provenance does not match the
  manifest.

### 2.2 Fixture classes

| Fixture | What it is | Used for | Mutation risk |
| --- | --- | --- | --- |
| **Recording shims** | Scripted stand-ins for `incus`, `systemctl`, `apt-get`/`apt-cache`/`dpkg`/`dpkg-query`, `podman`, `buildah`, `helm`, `kubectl`, `ollama`, `curl`, `sudo`, `cloudflared`, `tailscale`. Each appends `{argv, env-allowlist, stdin-hash, cwd}` to a JSONL trace and replays a scripted response. Scripts can inject exit codes, delays, partial output, or a block-until-signal via a FIFO. This extends Go's own `writeIncusFixture` pattern in `test/modes/packaged_test.go`. | M1–M8 T1 runs; fault injection | None |
| **Fake provider** | A small MCP Streamable HTTP server that speaks the provider plugin contract (`schemas/opute-provider-plugin.v1.json`) with scripted misbehavior: bad manifest, schema drift, slow or failing readiness, crash mid-activation, unauthorized callbacks. | M7 | None |
| **Real Go providers** | The pinned `opute-provider-{k3s,cloudflare,tailscale,ollama,hostos}` executables, unchanged, with shimmed external tools and without real credentials. | M7, M9 | None at T1 |
| **Disposable sandbox** | Two throwaway VMs cloned from one golden snapshot, with nested Incus, systemd, and optionally k3s. One VM serves Go and one serves Rust. They are destroyed after each run. | M3, M8, M12 T2 runs | Contained |
| **Clean machines** | Fresh Ubuntu LTS and Debian VMs (x64 and arm64) and a WSL2 Windows runner, with no prior Opute state. | M10 | Contained |
| **Real clients** | Headless `codex exec` (legacy handshake), an MCP 2026-07-28 client, `pkg/hostagentclient`, and the npm launcher. | M2, M10, M11 | None |
| **Platform staging** | A dedicated Rust validation identity enrolled in a non-production Platform. | M11 | Staging only |

A Go call that bypasses `PATH` or a `*_BINARY_PATH` override, for example by
using an absolute path or a socket API, cannot be recorded by a shim. The M0
inventory must list every such call. Its effects are then verifiable only at
T2, and the inventory records that explicitly.

### 2.3 Evidence tiers

| Tier | Runs where | Cadence | Proves |
| --- | --- | --- | --- |
| **T0** | Rust unit/contract tests plus Go focused tests as a reference | every commit | Internal correctness. It is **never** parity evidence on its own. |
| **T1** | Twin processes plus shims on one CI machine | every PR | Wire, CLI, catalog, admission, plan-trace, and state-shape parity; zero unintended external calls |
| **T2** | Twin disposable sandboxes | nightly, and required at milestone exit | Real external effects, cleanup parity, crash/restart on real resources |
| **T3** | Clean machines, real clients, Platform staging | at milestone exit (M10+) | Operator journey, published-artifact path, Platform boundary |
| **T4** | Rehearsal on isolated copies of real state | M12 | Conversion, rollback, soak |

### 2.4 Reuse Go's black-box suites as an oracle

Some Go suites already drive a binary from outside, so they run **unmodified**
against the Rust binary. That makes them free, independent parity checks:

| Go suite | Binary hook | Use |
| --- | --- | --- |
| `test/standalone` (HTTP contract, isolation/shutdown, invalid-config exit, `--transport` rejection) | `OPUTE_STANDALONE_BINARY` | M1–M3, then every PR |
| `npm/local-host-agent` canaries (`index.test.js`, published-readonly-canary) | `OPUTE_HOST_AGENT_BINARY` | M10 |
| `test/live` (`-tags=integration`, VM lifecycle, reset stack) | built Linux binary | M8b at T2 |
| `test/modes/packaged_test.go` | builds from source, so the harness must supply a binary override | M1 |

The in-process suites (`test/compliance`, `test/contract`, and the
`internal/hostmcp` tests) construct Go servers directly, so they cannot see a
Rust binary. M0 re-expresses each externally observable assertion they make as
a black-box harness scenario. Each one then keeps its Go test name as a
traceability anchor.

### 2.5 Harness self-validation (the harness must be able to fail)

A differential harness that always passes proves nothing. It must pass these
checks before it gates anything:

1. **Go-vs-Go is green and deterministic.** Run every scenario 20 times with
   Go on both sides. Any flake is a harness bug or a missing volatile-field
   declaration, and it is fixed before M1.
2. **Mutation canaries go red.** A set of deliberately broken Go builds (a
   patch overlay applied in the harness, never pushed to Go) must each be
   caught by the named scenario. Examples: one changed tool description, one
   extra tool, `401` changed to `403`, a missing `WWW-Authenticate`, a legacy
   method added to the bypass list, one extra `incus` argv token, one
   secret-bearing field left unredacted, one plan node state reordered. A
   canary that stays green is a harness gap.
3. **Missing evidence is a failure.** Deleting one evidence file, changing a
   provenance hash, or marking a live check `blocked` must make the verifier
   fail or report `unverified`. It must never report a pass.

### 2.6 Cross-cutting invariants that every milestone reruns

These scenarios join the suite as soon as the relevant surface exists. After
that they run on every PR:

| ID | Invariant | Check |
| --- | --- | --- |
| X1 | No listener before ready | Poll the port at 1 ms intervals during startup under every failure case; a failed startup never accepts a TCP connection on `/mcp`. |
| X2 | Rejected calls have no effects | For every rejected call, the shim trace is empty (or read-only verbs only), and no rows appear in `operations`, `plan_runs`, or `capability_invocations`. |
| X3 | Secret canary | Every write-only field receives a unique random canary. `grep -r` across the state dir (DB, `-wal`, `-shm`), logs, stdout/stderr, HTTP responses, and task events finds zero hits. Only the shim that legitimately receives the secret may see it. |
| X4 | Exact identity | `OPUTE_REMOTE_AGENT_ID` appears byte-for-byte in every place Go puts it. Hostnames and fingerprints never replace it. Whitespace-padded and ambiguous IDs fail the same way Go fails. |
| X5 | Catalog stability | Any change to the Rust catalog snapshot for any mode or provider matrix cell must come with a reviewed manifest update. |
| X6 | No phone-home | While the twin runs, egress is captured (network namespace or proxy log). Rust opens no connection that Go does not open. |

## 3. Milestones

Every milestone lists its **scope**, the **E2E validations** that must pass, and
its **exit gate**. The exit gate is a verifier run over the manifest subset
the milestone owns, plus every earlier milestone's subset. It is never a
manual checkbox.

---

### M0: Parity harness, inventory, and fail-closed verifier

**Closes:** 0.1, 0.2, 0.3, 0.4, 0.5. **Rust runtime code:** none.

**Status:** harness, inventory and verifier are implemented, and the M0 gate passes. See
[evidence/m0/README.md](../../../evidence/m0/README.md) for results, findings and the
explicit gaps that remain, including the owner review for task 0.5.

**Scope**

- Build the Go reference reproducibly from the pinned SHA. Record the binary
  hash and verify that SHA and tree match `baseline/source-lock.json`.
- Machine-readable inventory under `baseline/inventory/`:
  - `cli.json`: subcommands `standalone | serve | public-mcp | recipe
    {validate,apply,status} | provider … | help`, every flag, `--check`, exit
    codes, and usage text. It must include the flags the npm launcher passes
    (`--mode=standalone --transport=http`).
  - `env.json`: every `OPUTE_*`, `HOST_MCP_*`, and `MCP_AUTH_TOKEN` variable
    with its default, precedence (env file versus environment versus flag),
    and mode dependence. It covers mode-dependent defaults such as port 3014
    in standalone versus 3004 in platform, bind 127.0.0.1 versus 0.0.0.0, the
    instance root layout, and tenant `local`. It also covers retired
    variables that must be *rejected*, such as `OPUTE_REVERSE_TUNNEL` and
    `OPUTE_HOST_WS_URL`, and `OPUTE_TRANSPORT` values other than `http`.
  - `http.json`: routes, auth modes (bearer token and the OAuth authorization
    server: PRM, AS metadata, authorize, token, revoke, CIMD fetch with SSRF
    rejection), origin/DNS-rebinding rules, and the ADR 0011 legacy method
    set.
  - `catalog/<mode>/<provider-set>.json`: authenticated `tools/list` captures
    for each matrix cell (§4). Each capture records effect, approval,
    idempotency, resource bindings, `resourceCost`, conditional availability,
    and catalog revision, plus the prefixed-name variant
    (`OPUTE_MCP_PREFIX_TOOL_NAMES`).
  - `state.json`: every SQLite store and table (`operations`, `plan_runs`,
    `active_runtimes`, `provider_generations`, `capability_invocations`,
    `resource_registry`, `active_capabilities`; authz `clients`, `codes`,
    `tokens`; the sqlite-database domain files), its columns, migrations
    (including the `PRAGMA table_info`-driven column upgrades), and the
    file-mode expectations.
  - `effects.json`: every external command and API the Go agent reaches,
    tagged `shimmable` or `T2-only`.
  - `plans.json`: recipe schema versions, node states, retry, readiness,
    compensation, and the fixtures in `test/fixtures` and `plugins/*/recipes`.
  - `distribution.json`: release artifact names (`host-agent-linux-{x64,arm64}[.gz]`,
    `host-agent-windows-x64.gz`, provider binaries), `SHA256SUMS`, systemd
    units, sudoers, npm launcher download and verify logic.
- The harness (§2): twin runner, shims, fake provider, canonicalizer, typed
  masks, evidence bundle format.
- `parity-manifest.json` and a verifier (`make parity-verify`) that fails
  closed on missing, stale, malformed, or failing items. The verifier writes
  `verify` into `.agents/decisions/rust-cutover-parity-gate.json`.

**E2E validations**

1. Go-vs-Go: every captured scenario is green 20 times in a row (§2.5.1).
2. Mutation canaries: each one fails its named scenario (§2.5.2).
3. Verifier negative tests: missing, stale, and blocked evidence all fail (§2.5.3).
4. Inventory cross-check: every tool name in the Go dispatch registry, every
   `catalog.go` / `standalone.go` descriptor, and every `schemas/*.json` entry
   maps to an inventory item with an owner and a scenario. An unmapped item
   is a failing gap, not a warning.
5. Every conflict found between prose and typed behavior is recorded as an
   explicit item with a decision.

**Exit gate:** the verifier is green for the harness self-tests and reports
**every** Rust item as `unverified`, not missing. The inventory has zero
unowned surfaces.

---

### M1: Rust skeleton, CLI, configuration, lifecycle

**Closes:** 1.1, 1.3, 2.1. **Depends on:** M0.

**Status:** implemented; the `m1` gate passes. See
[evidence/m1/README.md](../../../evidence/m1/README.md).

**Scope:** workspace (binary plus only the contract, core, host, and server
modules that real edges justify); formatting, lint, test, and reproducible
build; CLI and config parsing with identical precedence; `--check`; ordered
startup and reverse-order shutdown; signal handling. `/mcp` serves nothing yet
beyond what M2 adds.

**E2E validations (T1)**

1. **CLI differential matrix:** every subcommand × flag × env combination in
   `cli.json` and `env.json`, including invalid ones. Compare exit code,
   normalized stdout/stderr, created files and their modes, and the absence
   of created files on failure.
2. **Fail-closed startup matrix:** missing, empty, or whitespace-padded
   `OPUTE_REMOTE_AGENT_ID`; bad mode; invalid `OPUTE_TRANSPORT`; retired
   `OPUTE_REVERSE_TUNNEL` / `OPUTE_HOST_WS_URL` / `OPUTE_CPC_TOKEN`; port in
   use; unwritable state dir; malformed env file. Each case must produce an
   identical exit and pass X1 (no listener).
3. **Defaults:** standalone binds `127.0.0.1:3014`, platform binds
   `0.0.0.0:3004`, instance-root paths match, and mutations are off by
   default. Check the actual sockets with `ss -ltnp`.
4. **Lifecycle:** fault-inject each startup stage (state open, authz open,
   catalog build, listener bind). Assert reverse-order cleanup with no
   leftover lock files, sockets, or child processes. SIGINT and SIGTERM
   during idle and during a slow shim call must produce the same exit code
   and the same shutdown log as Go.
5. **Go oracle:** `test/standalone` `TestStandaloneInvalidConfigurationExitsBeforeMCP`
   and `TestDeprecatedTransportFlagIsRejectOnly` pass against the Rust binary.
6. **Two agents on one machine:** Go and Rust run at the same time with
   distinct IDs, ports, tokens, and state. Neither reads the other's state,
   and neither infers its ID from the machine.

**Exit gate:** CLI, env, and lifecycle manifest items green; X1 and X4 green.

---

### M2: HTTP transport, authentication, MCP wire

**Closes:** 2.2. **Depends on:** M1.

**Status:** implemented; the `m2` gate passes. See
[evidence/m2/README.md](../../../evidence/m2/README.md). Three scope changes,
each recorded there:

- OAuth token issuance (`/oauth/authorize`, `/oauth/token`) follows the
  stricter `secure-oauth-issuance` change (D8), a declared divergence from Go.
  At the M2 merge Rust refused issuance with 501; that change replaces it,
  with its own Rust contract suite and Rust canaries in the `m2` gate (see
  [evidence/oauth-issuance/README.md](../../../evidence/oauth-issuance/README.md)).
  Bearer validation, metadata, revocation and the shared token store stay
  under Go parity.
- Item 5's packaged Go tests (`TestPackagedShapeStandaloneHTTPContract`,
  `TestStandaloneHTTPIsolationAndShutdown`) assert catalog contents and a
  task round trip, so they move to M4. The `test/compliance` assertions that
  M2 owns are black-box corpus scenarios.
- Item 6 uses the official go-sdk client as the real client. Codex and other
  third-party clients need the catalog and move to M3.

**Scope:** `/health`, `/mcp` Streamable HTTP, MCP `2026-07-28`
(`server/discover`, headers, `_meta` protocol keys), bearer auth, the OAuth
authorization server and its SQLite store, origin and localhost protection
(including the `OPUTE_MCP_DISABLE_LOCALHOST_PROTECTION` opt-in), the bounded
ADR 0011 legacy gate, the tool-name prefix option, `tasks/get` inline results,
`tasks/list` unsupported, and resources not advertised.

**E2E validations (T1, plus T3 for clients)**

1. **Wire corpus replay:** one versioned corpus (initial target: 500+ requests),
   replayed against both binaries. It combines hand-written cases with
   generated combinations: missing or wrong `Mcp-Method` and
   `MCP-Protocol-Version`; mismatched `_meta`; malformed JSON-RPC; batches;
   notifications; wrong `Content-Type` and `Accept`; oversized bodies; unknown
   methods; GET and DELETE on `/mcp`. Compare status, the header allowlist
   (`WWW-Authenticate`, `Content-Type`, MCP headers), and canonical bodies.
2. **Auth matrix:** no token, wrong token, correct token, `/health` without a
   token (always open), and token reuse after revoke. Run the full OAuth
   journeys over real HTTP: dynamic client via CIMD, PKCE S256 authorize, code
   exchange, code replay rejection, client credentials, revoke. Include SSRF
   rejection for loopback, link-local, and private CIMD URLs, and a
   non-loopback native redirect rejection.
3. **Legacy gate bound (ADR 0011):** flag off, so `initialize` is rejected.
   Flag on, so every listed legacy method is admitted without modern
   metadata, while `server/discover`, `tasks/*`, and every unlisted method are
   still validated. Every method in the inventory is exercised in both flag
   states. That covers the whole method list, not a sample.
4. **Origin and rebinding:** loopback origin, matching host origin, foreign
   origin, public `Host` header with and without the disable opt-in.
5. **Go oracle:** `test/standalone` `TestPackagedShapeStandaloneHTTPContract`
   and `TestStandaloneHTTPIsolationAndShutdown` pass against Rust. The
   `test/compliance` assertions (auth protects `/mcp` but not `/health`,
   `tasks/list` unsupported, `tasks/get` inline, resources not advertised,
   input-required round trip) pass as black-box scenarios.
6. **Real clients (T3):** a modern MCP client completes discover →
   `tools/list`. Headless `codex exec` connects with the legacy flag on and
   fails with the same error as Go with the flag off.
7. **Authz state:** the OAuth store written by Go opens in Rust, and Rust's
   store opens in Go (on copies), with identical accepted and rejected tokens.

**Exit gate:** wire corpus 100% green; X1 green; authz store cross-read green.

---

### M3: Catalog publication and read-only host tools

**Closes:** 1.2, 2.3 (read-only part). **Depends on:** M2.

**Scope:** one descriptor source generated from the pinned contracts and
schemas, which drives Rust validation, MCP publication, and parity snapshots;
`tools/list` for every mode × provider-set cell; catalog revision
computation; read-only tools (`get_host_info`, inventory and list tools, host
file inspection, probes, capacity and heartbeat projections).

**E2E validations**

1. **Catalog matrix (T1):** for each cell of §4, compare the authenticated
   `tools/list` by name set, then per tool: title, description, input and
   output schema (canonical), effect, approval, idempotency, resource edges,
   `resourceCost`, and prefixed names. **Also compare the catalog revision
   hash.** If the revision differs, the hashing input differs, and Platform
   would treat the catalog as changed.
2. **Descriptor determinism:** regenerate twice and diff; a changed pinned
   contract with no manifest update fails.
3. **Read-only calls (T1):** every read-only tool with valid, invalid, and
   edge-case arguments against shim fixtures (empty inventory, many
   instances, stopped instances, malformed `incus` JSON, shim timeout).
   Compare results and typed errors. X2 holds: the shim trace contains only
   read verbs.
4. **Real host (T2):** `get_host_info {}`, inventory, and capacity against the
   twin sandboxes on Ubuntu x64 and arm64 and WSL2. Compare field sets and the
   value types of host-dependent fields; compare stable fields (OS, kernel,
   distro, WSL classification) exactly.
5. **Default mutation denial:** every mutating tool in standalone mode with
   the gate closed gets an identical typed denial with an empty trace
   (preview of M4; cheap and high-value).

**Exit gate:** every catalog cell is identical; read-only scenarios green; X2
and X5 green.

**Status:** implemented; the `m3` gate passes. See
[evidence/m3/README.md](../../../evidence/m3/README.md). Scope changes, each
recorded there:

- Catalog cells are the four Go publishes without installed providers
  (standalone, standalone with the mutation gate open, platform, prefixed
  names). Provider cells stay the owned inventory gap for M7.
- Read-only tools in M3 are those that need neither resource binding nor
  workload admission: `get_capability_catalog`, `get_host_info`,
  `get_host_capacity`, `list_vms`, `detect_host_platform`. Resource-bound
  reads (`get_vm_info` and every tool with a declared resource argument) need
  M4 binding; `normal`-class reads (`inspect_host_file`,
  `probe_http_endpoint`, ...) are refused by Go's admission when workload
  enforcement is unverified, so they arrive with M4 admission; domain reads
  (Kubernetes, PostgreSQL, OCI, LLM, recipes, plans, operations) arrive with
  their domains. Until then each fails closed with a typed `not_implemented`
  capability error; `tools/list` is unaffected.
- Durable invocation evidence (`capability_invocations`) needs the
  schema-derived redaction of M5, and reservations need M4 admission. M3
  scenarios compare results, typed errors and command traces; the X2
  scenarios also check that rejected calls write no rows on either side.
- Real-host runs (T2: Ubuntu x64/arm64, WSL2) are not available in this
  environment; `get_host_info` and `detect_host_platform` are compared live
  on the CI host only.

---

### M4: Admission, resource identity, host resource control, tasks

**Closes:** 2.3 (admission), part of 3.2. **Depends on:** M3.

**Scope:** canonical resource URIs and kinds (`vm:` versus `container:`),
tenant scope, the mutation and approval gates, the storage-quota
enforceability admission (ADR 0010), the host resource coordinator
(normal/heavy/queued limits, min-memory and min-disk, fail-closed policy), the
reservation lease, the task registry, the `input_required` round trip, and
cancellation.

**E2E validations (T1)**

1. **Admission matrix:** every mutating tool × {gate closed, approval
   missing, wrong runtime kind, foreign tenant, unknown target, malformed
   URI, stale catalog revision, quota unenforceable}. The typed error must be
   identical, and X2 must hold with an empty effect trace.
2. **Coordinator saturation:** block shims on a FIFO so N calls hold slots,
   then issue N+k calls. Compare which calls queue, which get rejected, the
   error payloads, and the order in which slots are released. Set capacity
   env vars to small values so the test finishes in seconds.
3. **Task lifecycle:** the long-running call → `tasks/get` polling → terminal
   sequence is identical, including intermediate states and progress payloads.
   The `input_required` round trip matches `TestMCPInputRequiredTaskRoundTripOverHTTP`.
4. **Cancellation:** cancel before start, during a blocked shim, and after
   completion. Compare terminal state and the shim's received signal (the
   shim records SIGTERM/SIGKILL), and check that no orphaned child processes
   remain.
5. **Timeouts:** Go's `test/standalone/timeouts_test.go` expectations hold as
   black-box scenarios.

**Exit gate:** admission matrix fully green; zero orphans; X2 green.

**Status:** implemented; the `m4` gate passes, with two canaries
(C12-enforcement-fail-open, C14-late-result-overwrites-cancel) waived rather
than caught — pending owner review. See
[evidence/m4/README.md](../../../evidence/m4/README.md) for the waiver
rationale. Scope changes, each recorded there:

- **Admission matrix rows.** Gate closed and stale revision run over all 87
  non-read tools; missing, malformed, foreign-tenant and wrong-kind URIs over
  the 24 tools with a `uri` binding; unknown and mistyped Incus targets over the
  9 Incus-bound ones. Three rows move out of M4. *Approval missing* is
  Platform-owned: the agent has no approval gate of its own, and the Platform
  owns authorization. *Quota unenforceable* (ADR 0010) is checked inside
  `CreateVM`, not in admission, so it arrives with M6 provisioning. Cluster
  and host-service adoption needs the M6 domains and must never reach a real
  cluster. Where a tool binds several resource types, Go reports the error of
  the last type tried; Rust matches.
- **D12.** A task-aware call refused at binding or admission still gets a
  task, as in Go, but Rust writes no `operations` row. The scenarios exempt
  only the Go side (`x2GoGaps`), and the
  `refuse-before-operation-record` contract suite plus canaries prove the
  Rust behaviour.
- **Coordinator queueing.** Go's queued class (`AcquireClass`, `MaxQueued`)
  is not reachable from tool admission. Normal and heavy calls are admitted or
  refused with `host_capacity_saturated`, and the snapshot reports held
  slots under `reservations`. The N+k scenario holds two normal slots on a
  FIFO, refuses the third call twice, admits a control call, and admits again
  after release. `OPUTE_HOST_MAX_NORMAL_OPERATIONS=0` means "use the default"
  on both sides.
- **Cancellation is cooperative** in Go and in Rust. Cancelling before start,
  during a blocked shim, or after completion never signals the child. The
  work runs to completion, its late result is discarded, and the task stays
  `cancelled` (or `completed` when it had already finished). The shims record
  any SIGTERM, SIGINT or SIGHUP, plus pid files left by SIGKILL; both lists are
  empty, and zero orphans remain.
- **Timeouts.** `test/standalone/timeouts_test.go` asserts only harness
  deadlines: ready within 90 s and the process bounded at 3 min. Every parity
  scenario is stricter: ready within 30 s, and an unforced stop.
- **Normal-class reads.** `inspect_host_file` and `probe_http_endpoint` are
  implemented and pass admission like any other normal call. Domain reads
  (LLM, Kubernetes/Helm, PostgreSQL, OCI, recipes, plans, operations),
  `diagnose_bridge`, `discover_service_ingress` and
  `inspect_host_service(_supervisor)` stay `not_implemented` until their
  domain milestones.
- **T2 is not covered.** All evidence comes from T1 shim fixtures in WSL2;
  there was no real cgroup enforcement or real Incus.

---

### M5: Durable state and schema-derived redaction

**Status:** in progress, not done. Operations/plan storage, provider-generation
and invocation storage methods, task persistence, restart restoration and shared
schema redaction are implemented. The pinned Go legacy status projection is
preserved, accepted workers drain before stores close, and D12 refusals create
no operation row. The admitted-task cross-read matrix passes all 18 checks;
refused-task full-row comparisons pass with the approved D12 difference and
an independent Rust no-write invariant. `make parity-verify-m5` now fails closed
on every missing named validation and on stale regression evidence. Accepted
built-in calls now persist invocation audit envelopes and schema-projected
observations. The full database-shape corpus passes 420/420 checks, cross-read
passes 18/18, and older-release migration passes 6/6 against the current
candidate; all three are independently accepted by the M5 verifier.
Varied crash recovery, full secret sink sweeps, unknown projection, final state
shape evidence and fresh regression gates remain open. See
[evidence/m5/README.md](../../../evidence/m5/README.md) and its implementation
refresh for the verified scope.

**Closes:** 4.3. **Depends on:** M4.

**Recommendation:** keep the Rust SQLite schema, file layout, and file modes
**bit-compatible** with Go at the pinned revision, and treat any
reorganization as a later, separate change. The converter for task 4.4 then
becomes a *verified identity plus version check*. That is far easier to
prove, and it makes rollback symmetric. This needs an owner decision
(see §6).

**Scope:** stores listed in `state.json`, WAL and busy-timeout settings,
additive column migrations, operation, task, plan, generation, and observation
records, and redaction derived from the write-only schema markings.
Projections the schema does not mark must fail closed.

**E2E validations**

1. **Shape parity (T1):** after each scenario from M1–M4, dump both
   databases (`sqlite3 .schema` plus canonical row JSON with typed masks) and
   diff them. Row counts, statuses, and redacted fields must match.
2. **Cross-read (T1):** Go writes state, then Rust starts on a *copy* and
   answers status and list queries identically over MCP. Repeat in the other
   direction. The two never write concurrently.
3. **Crash injection (T1):** send `kill -9` at randomized points (about 200
   seeds) while operations, tasks, and plans are writing. Restart and compare
   the recovered state and the MCP-visible status with Go under the same seed
   and injection point, using a deterministic shim-driven crash point. There
   must be no torn records and no invented success.
4. **Secret canary X3** across every write-only field in every descriptor.
5. **Unknown projection:** an unmarked field in a result must be rejected or
   redacted exactly as Go does. It must never be persisted verbatim.
6. **Older-state migration:** a DB fixture from the previous released Go
   version (pre-column additions) is upgraded by Rust and by Go identically.

**Exit gate:** state diffs empty; crash corpus green; X3 green.

---

### M6: Recipes and the single plan executor

**Status:** in progress, not done. The plan executor (`internal/plan`
ported in full: schema, graph, assert, interpolate, runner), the
`host-recipe.v1` envelope (source loading, input resolution, host-local
restrictions, `ValidateHostAgentVersion`), plan/recipe evidence redaction,
and the full MCP surface (`validate_host_plan`, `run_host_plan`,
`get_host_plan_run`, `validate_host_local_recipe`, `run_host_local_recipe`)
are implemented and proven at the Rust unit-test level (171 tests, real
end-to-end runs against a durable store, `cargo fmt`/`clippy` clean), all
routed through the one `Runner` -- proven not by inspection but by a static
structural test mirroring Go's own architecture test. None of the four
T1 E2E validations below (recipe corpus, invalid corpus, execution traces,
restart mid-plan) have been run against the Go reference yet, no
`parity-verify-m6` gate exists, and the resource-reservation lease around a
launched run is not ported. See
[evidence/m6/README.md](../../../evidence/m6/README.md) for the full
breakdown of what is and is not done.

**Closes:** 4.2. **Depends on:** M5. **Can run in parallel with M7.**

**Scope:** recipe schema validation, interpolation, canonical hashing, plan
graph extraction, assertions and readiness, bounded retry, compensation,
recovery and reconcile, the `recipe validate|apply|status` CLI, and the
`host_recipe_run` / `plan_run` MCP paths.

**E2E validations (T1)**

1. **Recipe corpus:** every recipe in `plugins/*/recipes/**`,
   `test/fixtures/**`, and the private site-deploy recipe *shape* (a
   synthetic equivalent without credentials). Compare the validate outcome
   and **canonical hash** byte-for-byte.
2. **Invalid corpus:** generated mutations of each valid recipe (bad binding,
   unknown node, cycle, wrong schema version, stale catalog revision,
   duplicate IDs, and type errors in interpolation) must produce identical
   typed errors and pass X2.
3. **Execution traces:** for each executable recipe with shims, compare the
   **ordered node state transition log** and the shim trace. Include fault
   scripts: fail node *k* once (the retry succeeds), fail permanently (the
   compensation order must match), readiness never true (the timeout must
   match), and cancel mid-node.
4. **Restart mid-plan:** kill during node *k*, restart, and run reconcile.
   The resume, compensate, or stop decision and the final durable state must
   match.
5. **Structural check:** a static test fails if any provider path creates
   plan execution outside the single executor (mirrors Go's architecture
   tests).

**Exit gate:** hash and trace parity across the whole corpus; the restart
corpus is green.

---

### M7: Provider lifecycle with unchanged Go providers

**Closes:** 3.1, 3.2. **Depends on:** M5. **Can run in parallel with M6.**

**Scope:** manifest, schema, and dependency validation; install; candidate →
active → draining generations; generation-bound admission; callback routing
(`provider_host_service`, resource delegation); task bridge; readiness;
restore; forced and normal teardown; bounded, idempotent, reverse-order
disposal. The Rust core hosts the **pinned Go provider executables
unchanged**. That keeps the provider process boundary identical, and it means
M7 tests only the core's side of the boundary.

**E2E validations (T1)**

1. **Fake provider misbehavior matrix:** bad manifest, schema drift,
   dependency missing, readiness slow, fails, or flaps, crash during
   activation, catalog publication failure, unauthorized callback, callback
   using a stale generation. Compare the reported candidate state, the
   unchanged prior active catalog and revision, and the error payloads.
2. **In-flight replacement:** start a long task on generation *g1* (a
   FIFO-blocked shim), activate *g2*, finish the task, and assert that it
   completed on *g1*. New calls get *g2*'s revision, and *g1* drains and then
   disposes. The generation and revision sequence must be identical to Go.
3. **Disposal order:** a provider stack with dependencies is torn down in
   reverse order under both a normal and a forced teardown. Repeated teardown
   is idempotent. Check the process tree for zero leftover provider
   processes.
4. **Real Go providers:** each provider with shimmed externals goes through
   install → activate → provider tool call → teardown. The catalog cell for
   that provider set must match M3's capture exactly.
5. **Restart:** restart with active providers, then compare restored
   generations, catalog revision, and provider process relaunch behavior.

**Exit gate:** lifecycle matrix green for the fake provider and for all five
real providers; no orphaned provider processes.

---

### M8: Mutating domain operations, one domain at a time

**Closes:** 4.1. **Depends on:** M4–M7.

Go `internal/domain` is about 18k lines across eight domains, so M8 is split
into sub-milestones that each ship independently behind the same gates:

| Sub | Domain | Notable surfaces | T2 sandbox needs |
| --- | --- | --- | --- |
| 8a | host | files, services and systemd, exec, archives and artifacts, firewall, agent installation, public MCP / quick tunnel, WSL compact and lifecycle, Incus uninstall | systemd VM; WSL2 runner |
| 8b | incus | VM and container launch, lifecycle, images, stack, root disk, storage quota, static IP, ownership modes | nested Incus |
| 8c | kubernetes / cluster | k3s membership, helm install and template, exec, secrets, guest storage, exposure, cluster agent relay | k3s in sandbox |
| 8d | oci | build and push (podman/buildah), registry, registry storage, build-context staging | podman + local registry |
| 8e | postgres / sqlite | platform and standalone Postgres, relay, storage, SQL connector, SQLite database provisioning | Postgres container |
| 8f | llm / serving | Ollama, llama-server build and prerequisites, relay and gateway, serving assignment and ingress | CPU-only model stub |

**E2E validations per sub-milestone**

1. **Shim-trace parity (T1):** for every tool in the domain, run success,
   typed-failure, and partial-failure scripts. The argv trace must be
   identical, including order, flags, and stdin hash, and results must be
   identical.
2. **Effect parity (T2):** twin sandboxes from one snapshot. Run the same call
   sequence (create → mutate → read → delete), then diff the **external
   state**: `incus list/config show --format json`, systemd unit files and
   `systemctl show`, file trees (path, mode, owner, sha256), k8s objects
   (`kubectl get -o json` with typed masks), and the registry catalog.
3. **Cleanup parity (T2):** after teardown, each sandbox's diff against the
   golden snapshot is empty, or identical for Go and Rust where Go knowingly
   leaves residue, which the inventory must record.
4. **No unauthorized effects:** for the domain, every admission-rejected call
   from M4 is re-run in T2 and the sandbox diff stays empty.
5. **Go oracle:** for 8b, `test/live` `TestLiveVMCreateListDelete`,
   `TestLiveGetHostInfoIncus`, and `TestLiveResetIncusStackDeleteReconcileAndVerify`
   pass against Rust in the sandbox.

**Exit gate per domain:** T1 and T2 parity green; the domain's catalog tools
move from `unverified` to `pass` in the manifest.

---

### M9: Provider executables in Rust (optional, per provider)

**Closes:** 3.3 where a port is chosen. **Depends on:** M7, and M8 for the
domains a provider uses.

Providers are separate processes behind a versioned contract, so the Go
provider executables can keep running under a Rust core. Porting a provider
is **not required** for the core cutover, and the decision to port one is
made per provider (see §6). If one is ported, do it in ascending risk order:
`hostos` → `ollama` → `cloudflare` → `tailscale` → `k3s`.

**E2E validations:** under the *same Rust core*, swap the Go provider binary
for the Rust provider binary and compare the provider-boundary MCP traffic
(provider `tools/list`, calls, callbacks, events), shim traces, and T2 effects.
Also run the provider's own Go tests that drive the binary, such as
Tailscale's `mcp_wire_test.go` and `operator_drive_test.go` patterns, as black
boxes.

---

### M10: Packaging, installer, npm launcher, site

**Closes:** 5.1, 5.2. **Depends on:** M8 (M9 is optional).

**Scope:** release artifact names, gzip, and `SHA256SUMS`; systemd units
(`opute-host-agent.service`, `opute-bootstrap-mcp.service`) and sudoers;
installer; the npm launcher's download → verify → marker → spawn path; schema
export; generated site docs and catalog.

**E2E validations (T3)**

1. **Artifact contract:** the Rust release set has the same file names, the
   same compression, and a `SHA256SUMS` that verifies. `--version` and
   `-version` print the same format.
2. **Clean-machine install matrix:** Ubuntu LTS x64 and arm64, Debian, WSL2.
   Install from a locally served release base
   (`OPUTE_HOST_AGENT_RELEASE_BASE_URL`), start through systemd, run a
   read-only first call, stop, uninstall. Compare the unit status, installed
   paths and modes, and journal lines against Go on a twin VM.
3. **Upgrade and downgrade on the same identity:** Go → Rust → Go with state
   preserved (depends on the M5 recommendation). The explicit ID and token
   survive, and nothing is re-enrolled.
4. **npm launcher:** `index.test.js`, `local-npm-readonly-canary`, and the
   published-readonly-canary run against a locally packed tarball that points
   at the Rust artifacts. Also test a corrupted archive (checksum mismatch
   rejected), a concurrent-download lock, and the unsupported-platform message.
5. **Site:** `check-site` equivalents (catalog capture → generated docs →
   release-boundary and parity checks). The generated docs from the Rust
   catalog match Go's byte-for-byte, or the difference is a reviewed manifest
   item.

**Exit gate:** clean-machine journeys green on every platform; the upgrade
and downgrade round trip is green.

---

### M11: Real clients and Platform integration

**Closes:** 5.3. **Depends on:** M10.

**E2E validations (T3, staging only)**

1. A dedicated Rust validation identity (never a production ID) is enrolled
   in staging Platform. Registration, routing to the exact
   `OPUTE_REMOTE_AGENT_ID`, and authorization rejection for a foreign tenant
   are verified.
2. A scripted Platform-driven journey covers discover → list → read-only call
   → approved mutation in the sandbox → task polling → durable evidence.
   Compare task correlation IDs end to end.
3. **Semantic separation:** the Host Agent response never asserts request
   satisfaction (mirrors `TestHostAgentNeverAssertsSatisfaction`), and
   Platform-side records attribute the semantic outcome to Platform.
4. **Client first-success:** `codex exec` over WSL2 (mirrors
   `TestCodexWSLNonInteractiveE2E`), a modern MCP client, and
   `pkg/hostagentclient`, each starting from a clean machine using only the
   published tutorial steps.

**Exit gate:** all journeys green in staging, with evidence bound to the
artifact hashes from M10.

---

### M12: State conversion, soak, and cutover rehearsal

**Closes:** 4.4, 6.1, 6.2. **Depends on:** M11.

**E2E validations (T4, on isolated copies only)**

1. **Store inventory at the selected Go revision.** If Go moved on, rebase the
   manifest first and rerun the affected milestones.
2. **Converter dry run** on checksummed read-only copies of representative
   real state (including in-flight and failed operations). Reconcile counts,
   identities, hashes, terminal and pending states, revisions, file modes,
   and redaction. Any ambiguous record blocks the rehearsal.
3. **Forward rehearsal:** stop Go → snapshot → convert → start Rust → query
   over MCP → restart → query again. Results must be identical to Go's
   pre-stop answers.
4. **Rollback rehearsal:** stop Rust → restore or convert back → start Go.
   Rust-era accepted operations are preserved, or rollback refuses and halts.
   There is never a silent drop. Two writers on one state directory must be
   prevented, which means the lock is tested.
5. **Soak:** 72 hours or more in twin sandboxes under a synthetic mixed
   workload (read-only, mutations, plans, provider replacement, periodic
   restarts). Compare error rates, latency percentiles, RSS and FD growth,
   and final state parity.
6. **In-flight policy drill:** cut over with long tasks running and verify
   the documented drain or refuse behavior.

**Exit gate:** full-manifest `make parity-verify` green, with no `unverified`
items except ones the owners explicitly scope out through a separate
OpenSpec change.

---

### M13: Cutover decision, then retirement as a separate decision

**Closes:** 6.3, and later 6.4. The Host Agent, Platform, release, and
deployment owners review the complete manifest. Production selection happens
through the normal rollout path, with Go kept as the rollback candidate
throughout the agreed window. Go retirement, and ownership of releases and the
website, are later decisions that need their own evidence.

## 4. The catalog and provider matrix

Catalog parity is checked per cell and never across cells, because different
provider sets legitimately expose different catalogs:

| Mode \ providers | none | incus | incus + k3s | incus + cloudflare | incus + tailscale | incus + ollama | incus + hostos | all |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| standalone (gate off) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| standalone (gate on) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| platform | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| × prefixed names | sampled: one column per mode | | | | | | | |

M0 confirms which cells Go actually supports. An unsupported cell is recorded
as such, with the Go evidence, rather than silently dropped.

## 5. What a milestone's evidence bundle contains

```text
evidence/<milestone>/<run-id>/
  provenance.json      go sha/tree, rust commit, binary + fixture hashes
  matrix.json          mode × provider cells exercised
  scenarios/<id>/
    go/   rust/        raw observations (http, stdout, trace.jsonl, db dump)
    diff.json          canonical diff after typed masks (empty = pass)
  canaries.json        mutation-canary results (must all be red)
  summary.json         pass / fail / unverified per manifest item
```

The verifier reads only `summary.json` plus `provenance.json`, and it
re-checks their hashes against the raw files. A hand-edited summary fails.

## 6. Decisions needed from owners before or during implementation

| # | Decision | Recommendation | Why |
| --- | --- | --- | --- |
| D1 | Keep the SQLite schema and layout bit-compatible for cutover (M5) | **Yes** | This turns conversion into a verified identity, makes rollback symmetric, and removes the largest cutover risk. Any reorganization can be a separate change later. |
| D2 | Port provider executables to Rust (M9) | **Defer, per provider** | They sit behind a versioned process contract. Porting them is not needed to prove core parity, and it adds five more parity surfaces. |
| D3 | `host-agent-windows-x64.gz` is built by Go `make artifacts`, but the spec limits native execution to Linux and WSL2, and the npm launcher rejects Windows | **Record as a parity item and ask the owner** | Dropping the artifact silently would be a removed capability. Shipping it would need its own contract. |
| D4 | T2 sandbox infrastructure (where the nested-Incus VMs run) | A dedicated, disposable pool with no shared Incus, Kubernetes, or tunnel state | `AGENTS.md` forbids mutating shared runtime to validate. |
| D5 | Staging Platform identity for M11 | A dedicated `parity-rs-*` ID and tenant | Spec: distinct identity, never inferred. |
| D6 | Go rebase cadence while Rust is built | Rebase at each milestone exit, not continuously | This bounds churn and keeps evidence attributable. |
| D7 | Observable legacy surface ([legacy-inventory.md](legacy-inventory.md) class O) | Retire it in Go first through a separate OpenSpec change, then rebase | Rust never builds shims that will be deleted anyway. The parity principle holds because Rust still matches Go exactly, just a newer Go. |
| D8 | OAuth token issuance | **Decided (owner, 2026-10-01): Rust diverges.** Rust implements the stricter [`oauth-issuance`](../secure-oauth-issuance/specs/oauth-issuance/spec.md) contract; Go is not changed. Every affected Go-vs-Rust comparison declares the divergence by citing D8, and a Rust contract suite verifies the behaviour | Issuance must be backed by a credential or operator approval. The owner approved deviating from Go where the deviation is an improvement. |
| D9 | go-sdk `MCPGODEBUG` compatibility flags (finding F-7) | **Do not reproduce; ask the owner** | The Go agent inherits SDK switches (`disablelocalhostprotection`, `allowsessionsinstateless`, `disablecontenttypecheck`, ...) from any process environment. They are undocumented, not part of the Host Agent contract, and one of them disables the DNS-rebinding guard. Rust implements the default behaviour, which is identical to Go with the variable unset. |
| D10 | Standalone mutation gate | **Decided (owner, 2026-10-02): Rust diverges.** With standalone mutations disabled, Rust runs only tools whose catalog effect is `read` ([`standalone-read-only-gate`](../standalone-read-only-gate/specs/standalone-read-only-gate/spec.md)); Go is not changed. The catalog stays under Go parity, and a Rust contract suite with canaries verifies the gate | The gate derives from the same effect classification clients see, and fails closed when an effect is not known to be `read`. |
| D11 | Audit writes for rejected calls | **Decided (owner, 2026-10-02): Rust diverges.** The enumerated invalid MCP requests and closed standalone gate leave no audit writes ([`reject-without-audit-writes`](../reject-without-audit-writes/specs/reject-without-audit-writes/spec.md)); Go is not changed. The invocation count is the only declared state difference, and the Rust contract fingerprints every database row before and after rejection and restart | Rejected work must produce neither execution effects nor durable audit writes. Startup provisioning, accepted calls and D8 OAuth auditing keep their existing contracts. |
| D12 | Operation record for task-aware calls refused at binding or admission | **Decided (owner, 2026-10-02): Rust diverges.** Go's `createAsyncTask` persists an `operations` row before resolving the binding and admitting the call (`server.go:1642-1652`); Rust refuses first and records nothing durable ([`refuse-before-operation-record`](../refuse-before-operation-record/specs/refuse-before-operation-record/spec.md)). The task wire (`working`, then `completed` with the same typed error) is unchanged. The `operations` row count is the only declared difference, and a Rust contract suite with canaries verifies it | A refused call must not leave a durable operation that never ran; this extends X2 to the task path. |
| D13 | Unmarked-field projection (M5) | **Decided (owner, 2026-10-04): approve the stricter durable-projection contract, implemented as two named policies.** The owner chose the long-term-correct behavior over carrying Go's gap forward, then a second finding sharpened the implementation: Go's pinned `redactTaskResult` already runs a task-aware tool's *delivered* result through the same schema projection as its durable copy, so a single stricter function would also have hidden open-schema content from a caller's own `tasks/get` result -- a real functionality loss with no security benefit, since that caller already owns the data. `crates/host-agent/src/evidence.rs` now exposes `redact_for_delivery` (Go's original behavior, unchanged: only `writeOnly` is ever hidden from a caller) and `redact_for_storage` (D13's stricter, fail-closed default: an unmarked/open-schema value -- no `properties` entry, no typed `additionalProperties`, including a bare `additionalProperties: true` -- is redacted before it reaches a durable sink). `redact_task_result` (what `tasks/get` returns, live and after restart) uses delivery; `redact_task_args` and the `capability_invocations` audit row use storage. The Go agent is unchanged. Declared and specified in [`redact-unmarked-projections`](../redact-unmarked-projections/proposal.md) (new `evidence-redaction` capability), verified by its own contract suite with Rust canaries, same shape as D8. See [the owner decision record](../../../evidence/m5/unknown-projection/owner-decision.md) for the two options weighed. | Matching Go and Rust alone did not satisfy the current never-persist-unmarked-field text, and the owner judged Go's gap not worth preserving. A single blanket policy would have silently traded a storage-only hardening for a caller-visible regression; splitting by destination (delivered vs. durable) gets the security improvement without that cost. Changing durable retention (not what a capability accepts or returns) is exactly the kind of improvement D8 already set precedent for declaring rather than silently porting. |

## 7. Risks this plan specifically mitigates

| Risk | Mitigation in this plan |
| --- | --- |
| "Green but wrong": the harness masks real differences | Typed masks only, shape-checked masks, mutation canaries (§2.5) |
| Hidden side effects on rejected calls | X2 on every PR, and T2 re-runs of rejections (M8.4) |
| Secret leakage via WAL, logs, or errors | X3 canary across every sink, on every PR |
| Catalog revision drift breaks Platform | M3 compares the revision hash, not only tool names |
| Unshimmable effects escape T1 | M0 `effects.json` tags them `T2-only`; the verifier requires T2 evidence for them |
| Go moves during the port | Provenance binding; a stale SHA fails the verifier (D6) |
| Cutover data loss | D1, the M12 forward and rollback rehearsal, and the two-writer lock test |
