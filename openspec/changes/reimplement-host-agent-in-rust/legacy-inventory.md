# Legacy, compatibility, and structural debt inventory

This inventory lists the legacy paths, compatibility shims, workarounds, and
structural choices in the pinned Go Host Agent
(`ace7013df17528fee1bed13a1d70a132d6c5eb9b`). For each one it says whether the
Rust port can leave it behind. File references point into that Go tree.

This change is a pure refactor, so the rule is strict:

- **If nothing outside the process can observe it, Rust does not rebuild
  it.** The parity harness in [milestones.md](milestones.md) proves that
  dropping it changed nothing.
- **If a client, provider, recipe, operator, or durable-state reader can
  observe it, this change cannot drop it.** It needs its own OpenSpec change
  and an owner decision, plus evidence that nobody still uses it.

The inventory is a starting list built from source markers (`legacy`,
`compat`, `deprecated`, `retired`, `residual`, `fallback`) and a structural
read. M0 must fold every item into `baseline/inventory/` with an owner.

## 1. Classes

| Class | Meaning | What Rust does | Who decides |
| --- | --- | --- | --- |
| **I: Internal** | Go-internal structure or mechanism with no observable effect | Does not rebuild it; parity scenarios prove equivalence | This repository (design review) |
| **O: Observable legacy** | Legacy wire, schema, argument, default, or state behavior that someone outside could depend on | Keeps it until it is retired in a separate OpenSpec change | Host Agent owner, and the Platform owner for anything Platform calls |
| **K: Keep** | Looks like legacy but is a deliberate fail-closed guard or a live contract | Keeps it; it is not debt | No change |

## 2. Recommended strategy for class O: retire in Go first

```text
 Option A: carry into Rust            Option B: retire in Go first (recommended)
 ───────────────────────────          ─────────────────────────────────────────
 Rust implements every alias,         1. separate OpenSpec change retires the
 wrapper, and legacy default             O items in Go
       │                              2. Platform/clients migrated, evidence
 parity harness proves it                captured (call logs show zero use)
       │                              3. Go release ships without them
 later: second change to retire       4. rebase baseline/source-lock.json
 them in Rust too                     5. Rust never implements them
```

Option B keeps the parity principle intact, because Rust still matches Go
exactly, just a newer Go. It also shrinks the Rust surface before the most
expensive milestones (M3–M8). With Option A, every legacy path is built and
tested twice and then deleted anyway. The cost of Option B is one Go release
plus a baseline rebase, which D6 in milestones.md already schedules at
milestone boundaries.

If the evidence for an O item is not available in time, the item falls back
to Option A. It is carried into Rust and never dropped by guesswork.

## 3. Class I: internal debt Rust does not reproduce

These items are free wins. Each is replaced by the single mechanism named in
design.md's simplification rules. The **Parity guard** column names the
harness evidence that proves the replacement is invisible.

### 3.1 Catalog and descriptor authority

| ID | Item | Go evidence | Rust replacement | Parity guard |
| --- | --- | --- | --- | --- |
| I-1 | **At least six overlapping descriptor sources:** hand-written `catalog.go` definitions, the `schemas/all-tools.json` snapshot, 106 hand-written `StandaloneToolDefinitions`, `schemas/standalone-tools.json`, `internal/contract/toolname` constants, and per-family `schemas/*.v1.json` | `internal/tools/catalog.go`, `internal/tools/standalone.go:350`, `internal/contract/toolname/names.go` | One versioned descriptor source generates decoding, publication, and the parity snapshot (task 1.2) | M3 catalog matrix, including the revision hash |
| I-2 | **Runtime patching of the checked-in snapshot:** at catalog assembly, entries in `all-tools.json` are deleted and replaced with inline local-LLM definitions, because "the checked-in schema snapshots may still contain retired local-LLM definitions" | `internal/tools/catalog.go:1089-1105` | Generate the correct descriptors directly; never patch at runtime | M3 |
| I-3 | **Descriptions duplicated by hand** between the platform and standalone definitions (for example `cleanup_container_storage`) | `catalog.go:480` vs `standalone.go:505` | Covered by I-1 | M3 catches existing drift, which becomes a recorded Go defect |
| I-4 | **Catalog authority lives in another repository:** `make export-schemas` runs a TypeScript script in `../opute` and writes into a stale `../opute-host-agent/schemas` path | Go `Makefile` `export-schemas` | Descriptors are generated in this repository from the pinned contracts; the Platform consumes them rather than producing them | M0 inventory records the current flow; M10 site parity |
| I-5 | **Residual effect table:** the part of the pre-W8 name-keyed effect table that remains, kept alive by a contract test | `internal/tools/capability.go:340-360`, `ResidualEffectTableNames` | Effect is required on every descriptor, including transport- and provider-owned names | M4 admission matrix (effect-gated approval) |
| I-6 | **Residual task-mode table** kept in step with registrations by a test | `internal/tasks/registry.go:575` | Task mode is a required descriptor field | M4 task lifecycle |
| I-7 | **`RegisteredResourceCost` / `DefaultCostForClass` fallback** for built-ins that predate resource-cost metadata | `internal/tools/registry.go:126`, `internal/resource/service.go:247` | Every descriptor declares `resourceCost`; the values are generated from today's effective defaults so they stay identical | M3 (`resourceCost` compared), M4 saturation |

### 3.2 Capability execution path

| ID | Item | Go evidence | Rust replacement | Parity guard |
| --- | --- | --- | --- | --- |
| I-8 | **Every built-in runs through `NewLegacyAdapter`:** 119 registered dispatch handlers and **0** native `RegisterCapabilityModule` callers. The "migrate to native capability modules" plan never landed. | `internal/capability/adapters.go:51-78`, `internal/hostmcp/server.go:394,1319` | One capability shape: typed input, typed output, and an owned observation validator | M4, M5 (observation rows), M8 |
| I-9 | **`PassThroughObservation`** is a documented "narrow compatibility adapter" that records structured content without typed interpretation | `internal/capability/capability.go:79` | Typed observations per capability. **Caution:** the persisted observation JSON must stay byte-compatible if D1 is chosen. | M5 state diff |
| I-10 | **`ensureLegacyCapabilityForDescriptor`** lazily creates adapters during publication | `internal/hostmcp/server.go:1208,1266,1319` | Adapters are bound once, at composition time | M7 lifecycle |
| I-11 | **117 `init()` functions** fill the global `toolHandlers` map. That is a hidden global registry, which design.md forbids. | `internal/tools/registry.go:59` | Explicit registration in the composition root | M1 lifecycle, M3 |
| I-12 | **Service-locator handler signature:** every handler receives the whole `*hostagent.Service`, so any tool can reach any domain | `internal/tools/registry.go` `ToolHandler` | Each capability receives only the typed seams it declares | Structural test (no cross-domain access) |
| I-13 | **`dispatch_unassigned.go` parking lot:** tools "parked here rather than guessed into a domain" pending a partition that never finished | `internal/tools/dispatch_unassigned.go:20-23` | Each tool is placed in its owning domain | M3, M8 |
| I-14 | **`internal/tools` is a god package** combining catalog, dispatch for eight domains, effect tables, the standalone contract, and schema helpers (about 5k lines) | `internal/tools/*` | Split along the design's contract, core, and host owners | Architecture test |
| I-15 | **`hostmcp.Server` is a 2.1k-line monolith** combining MCP publication, provider install, candidate, teardown, and restore, recipes, tunnels, sessions, and reservations | `internal/hostmcp/server.go` plus about 20 sibling files | Transport adapter, catalog, provider lifecycle, and plan runner kept separate, per design.md | M2, M6, M7 |
| I-16 | **`vmName` → `name` shim inside schema validation:** the adapter copies the alias before JSON-schema validation because dispatch accepts either spelling | `internal/capability/adapters.go:128-140` | Normalize once at admission. The alias itself is O-5. | M4 admission matrix |

### 3.3 Configuration and process

| ID | Item | Go evidence | Rust replacement | Parity guard |
| --- | --- | --- | --- | --- |
| I-17 | **The process environment is the config bus:** the CLI resolves flags and then calls `os.Setenv` (`--env`, mode, transport) so that `config.Load()` can read them back | `internal/cli/cli.go:542,563-564` | Parse once into an immutable config value and pass it explicitly. Children that need env receive an explicit env. | M1 CLI/env matrix (precedence identical) |
| I-18 | **Environment read in about 17 packages outside `internal/config`**, including domain code, fingerprint, heartbeat, and `pkg/hostplatform` | `grep os.Getenv` list in M0 | All inputs flow from the config value; tests set config, not env | M1 env matrix covers every variable |
| I-19 | **Duplicated `envOr` helpers** (`config.envOr`, `hostruntime.EnvOr`, and others) | `internal/config/config.go:395`, `internal/hostruntime/shared.go:244` | One typed loader | M1 |
| I-20 | **The `cordis` mini-framework** (Service, Plugin, Fiber, Effect, `Inject()` keys) acts as an inversion-of-control container for lifecycle | `internal/cordis/context.go` | Plain ordered composition with RAII/`Drop` plus explicit async disposal. Keep the *semantics*: reverse-order, bounded, idempotent disposal. | M1.4 lifecycle fault injection, M7 disposal order |

### 3.4 Durable state mechanics

| ID | Item | Go evidence | Rust replacement | Parity guard |
| --- | --- | --- | --- | --- |
| I-21 | **Ad-hoc `PRAGMA table_info` column-scan migrations** with no schema version table; `ensureTableColumn` and `ensurePlanRecipeColumn` are copy-pasted | `internal/state/store.go:228,407-450` | Versioned, ordered migrations whose end schema is identical to Go's (D1). Adding a version marker is itself observable to Go on rollback, so it needs a check that Go ignores unknown tables. | M5 schema diff, M12 rollback |
| I-22 | **Type aliases for source compatibility:** the `ActiveRuntimeRecord` alias and the `Runtime` field on `ActiveCapabilityRecord` | `internal/state/store.go:51,100` | Not needed; there are no Go source callers in Rust | none needed |
| I-23 | **The `active_runtimes` → `active_capabilities` copy runs on every start**, using `INSERT OR IGNORE`, and the old table is never dropped | `internal/state/store.go:369-400` | One migration step. Whether old state directories exist at all is O-17. | M5 older-state fixture |
| I-24 | **Two separate SQLite files and stores** (state and authz), each with its own pool and PRAGMA workaround | `internal/state/store.go:111`, `internal/authz/store.go:62` | Keep both files for D1 compatibility, but share one open, PRAGMA, and migration helper | M2.7, M5 |

### 3.5 Platform and portability code

| ID | Item | Go evidence | Rust replacement | Parity guard |
| --- | --- | --- | --- | --- |
| I-25 | **Windows and Darwin code paths** (`runner_windows.go`, `host_lock_windows.go`, `disk_windows.go`, `request_lock_windows.go`, `collectDarwin`/`collectWindows`, and many `runtime.GOOS != "linux"` guards) although native execution is Linux/WSL2 only | listed paths | Build Linux only. **Blocked on D3:** if the Windows artifact stays, so does this code. | M10 artifact contract |
| I-26 | **Per-tool `runtime.GOOS != "linux"` guards** repeated in domain functions | `internal/domain/oci/*.go`, `internal/domain/incus/incus_container.go:60,642` | Gate once at the platform seam. The error text is observable only on non-Linux hosts, so it follows D3. | M3 read-only matrix |

## 4. Class O: observable legacy (drop only through a separate retirement change)

**Evidence needed to retire** is what the retirement change must show. In most
cases it is Platform and client call logs covering a representative window,
plus a search of the recipe and deploy repositories. This session could not
read the Platform repository (`wunderous/opute`), so none of that evidence has
been gathered yet.

### 4.1 Tools and operations

| ID | Item | Go evidence | Evidence needed to retire | Notes |
| --- | --- | --- | --- | --- |
| O-1 | **`ensure_local_llm_k3s_proxy` / `remove_local_llm_k3s_proxy`**, self-described as "LEGACY: … Prefer `ensure_local_llm_relay` plus generic Traefik route/domain tools" | `internal/tools/standalone.go:435,443`, `internal/domain/llm/gateway.go:96` | Zero calls; Platform and dogfood migrated to relay plus route tools | Also removes the `opute-llm` defaults (O-9) |
| O-2 | **Local-LLM tools published as "LEGACY compatibility wrapper … prefer runtime-recipe.v1"** (`check_local_llm_prerequisites`, `install_local_llm_model`, `start_local_llm_runtime`, `configure_local_llm_*`, `probe_local_llm`, `stop_local_llm_runtime`, `remove_local_llm_model`, and others) | `internal/tools/catalog.go:950-1107` | runtime-recipe.v1 covers every use; zero wrapper calls | The largest O item by surface. **The wire label is not proof of intent:** see G-1, which shows the same label reaches tools that are *not* legacy. Decide per tool. |
| O-3 | **`network-overlay.v1` capability and 14 `opute.capability.network-overlay.*` alias operations** ("deprecated: fan-out alias / migration only", ADR 0016) | `contracts/capability/capability.go:9,46-60`, `internal/recipe/recipe.go:314`, `plugins/tunneling/tailscale/.../main.go:375` | All recipes and callers use the three-seam IDs (mesh-runtime, mesh-membership, private-mesh, public-ingress) | Affects the provider catalog cells (M3, M7) |
| O-4 | **Provider-owned Cloudflare tool names kept in "compatibility exports for one migration period"** (`ensure_cloudflared_tunnel`, `install_cloudflared_connector`, and others) | `internal/tools/catalog.go:26-34` | The migration period has ended; exports and site docs no longer list them | These are filtered out of built-ins already; only the exported artifacts and docs change |

### 4.2 Argument aliases and legacy values

| ID | Item | Go evidence | Evidence needed to retire |
| --- | --- | --- | --- |
| O-5 | **`vmName` alias for `name`** (about 121 references), plus `vmName` "compatibility input; never an execution target" on Kubernetes arguments | `internal/capability/adapters.go:128`, `internal/domain/kubernetes/generic.go:15-31`, `secret.go:17` | Platform sends canonical `name` and target URIs everywhere |
| O-6 | **`numCtx` alias for `contextSize`** | `internal/tools/catalog.go:976,990,1022`, `dispatch_llm.go:141` | Zero `numCtx` in calls and recipes |
| O-7 | **Deprecated relay fields `RelayToken` and `AllowedSourceIP`** alongside `IncomingToken` and `AllowedSourceCIDRs` | `internal/domain/llm/gateway.go:20-22` | Zero use; note these are secret-bearing, so redaction parity (X3) covers both spellings until retirement |
| O-8 | **`buildkit` and `buildah` accepted as "legacy build-only values"**; Podman is the only real adapter | `internal/domain/oci/builder.go:20`, `build_and_push.go:105,181` | Zero use; retirement turns them into typed validation errors |
| O-9 | **Implicit legacy defaults when callers omit neutral fields:** relay namespace `opute-llm` ("one-release default"), Postgres relay device `opute-platform-postgres-rw` ("legacy in-process callers"), and Postgres database and secret defaults | `internal/domain/llm/relay.go:435`, `postgres/platform_postgres_relay.go:491`, `platform_postgres.go:47` | Callers always send explicit values; the retirement makes the fields required |
| O-10 | **Cloudflare legacy host alias:** `placement=host` silently derives `connector=host`, and there is a separate `connectorDeleteLegacySchema` that accepts deletes without `targetUri` | `plugins/tunneling/cloudflare/.../main.go:162-166,276-283,381,412` | Callers send `connector` and `targetUri`; this is a provider-contract change |
| O-11 | **An activation recipe with empty or absent `runtime.capabilities` publishes the full provider surface** ("legacy activates") | `internal/hostmcp/recipe_run.go:250-254` | Every activation recipe declares capabilities. Retiring this makes the allowlist required, which fails closed. |
| O-12 | **`MemoryFreeBytes` "backward-compatible alias for MemAvailable"** in the capacity projection | `internal/heartbeat/capacity.go:21` | No consumer reads it (check the Platform heartbeat and capacity readers) |

### 4.3 CLI, config, and wire

| ID | Item | Go evidence | Evidence needed to retire |
| --- | --- | --- | --- |
| O-13 | **Implicit `serve` when the first argument is a flag** ("historical implicit command behavior") | `internal/cli/cli.go:55-66` | Systemd units, installer, npm launcher, and docs all pass an explicit subcommand. Today the npm launcher passes **only flags** (`--mode=standalone --transport=http`), so the launcher must change first. |
| O-14 | **`standalone` subcommand as an alias of `serve --mode=standalone`** | `internal/cli/cli.go:510-512` | Same as O-13; keep one spelling |
| O-15 | **`OPUTE_INFRA_PROVIDER_ID`**, where any value other than `incus` is an error; the knob is vestigial | `internal/config/config.go:264-270` | No deployment sets a non-default value. Retiring it means the variable is ignored or rejected, which needs a decision. |
| O-16 | **`run_host_command` / workload command as one opaque `bash -lc` string**, "for compatibility with the existing host-command contract" | `internal/domain/host/host_service.go:70-80` | A typed argv replacement exists and callers have migrated. This is a **security-motivated** retirement, not only cleanup. |
| O-17 | **Old state upgrade paths:** "very old standalone state directories" lacking columns, `active_runtimes`, and pre-binding invocation rows | `internal/state/store.go:228,369-385` | The deployed fleet's state versions are known, and every one is at or above a declared floor. Then Rust needs only the floor's schema (pairs with D1). |
| O-18 | **Retired variables `OPUTE_HOST_WS_URL`, `OPUTE_CPC_TOKEN`, `OPUTE_REMOTE_AGENT_AUTH_TOKEN`, `OPUTE_ONBOARDING_*`, and `OPUTE_REVERSE_TUNNEL` are still read, in order to reject them** | `internal/config/config.go:272-283` | See K-2. The *rejection* stays; this item exists only so nobody "cleans it up" into silent acceptance. |

### 4.4 Architectural scope questions (observable, not simple aliases)

| ID | Item | Go evidence | Question for the owner |
| --- | --- | --- | --- |
| O-19 | **Platform-specific Postgres (CloudNativePG) and relay tools inside the "product-neutral kernel"**, including in-place upgrade logic for a "legacy cluster … running the pre-budget manifest" | `internal/domain/postgres/platform_postgres*.go` (about 1.2k lines), ADR 0008 | Should this move behind a provider seam (like k3s and Cloudflare) in a separate change? The migration-era in-place upgrade branch could be retired once every cluster is past the budgeted manifest. |
| O-20 | **Two parallel public profiles:** the platform catalog versus the standalone contract, with 96 `experimental`, 21 `stable`, and 2 `legacy` support levels in `standalone-tools.json` | `schemas/standalone-tools.json`, `internal/tools/standalone.go:34-40` | Keep two profiles? Promote or retire the two `legacy` entries? The support levels are published and therefore observable. |
| O-21 | **Windows release artifact** (`host-agent-windows-x64.gz`) while the spec and launcher support Linux and WSL2 only | Go `Makefile` `build-windows-x64` | Same as D3 in milestones.md. Retiring it unlocks I-25 and I-26. |

## 5. Apparent Go defects found during this inventory

These items are not legacy, but they matter for parity. Under the design's
rule, each one needs a decision: reproduce it exactly in Rust, or fix it in
both Go and Rust through a recorded, evidenced defect. The parity harness must
not silently "fix" it.

| ID | Defect | Go evidence | Observable effect |
| --- | --- | --- | --- |
| G-1 | `appendLocalLLMDefinitions` computes a tailored description and a typed `models` output schema per tool, then discards them. It filters every local-LLM name out of `defs` and re-appends each one with the generic description `"LEGACY compatibility wrapper for a local runtime operation; prefer runtime-recipe.v1"` and `outputSchema: {type: object}`. Both the platform catalog (`catalog.go:113`) and the standalone catalog (`standalone.go:547`) go through it, so the standalone definitions' own descriptions (for example `ensure_local_llm_relay` at `standalone.go:433`, the path O-1 tells callers to *prefer*) are overwritten too. | `internal/tools/catalog.go:1044-1107` | Every local-LLM tool, including the recommended relay tools, is advertised to clients and LLMs as LEGACY with an untyped output. The tailored text at `catalog.go:1047-1066` is dead code. |

M0 should confirm G-1 against a live `tools/list` capture before anyone acts
on it.

## 6. Class K: looks like legacy, but keep

| ID | Item | Go evidence | Why it stays |
| --- | --- | --- | --- |
| K-1 | **Bounded legacy handshake** (`OPUTE_MCP_ALLOW_LEGACY_HANDSHAKE`, default off) | `internal/transport/discover.go:52-100`, ADR 0011 | This is an accepted ADR with an enumerated method list. Codex and Cursor depend on it, and the conformance plan validates through it. Retiring it requires those clients to support `2026-07-28`. |
| K-2 | **Rejection of retired variables** (O-18) and of `OPUTE_TRANSPORT` values other than `http` | `internal/config/config.go:260-283` | These are fail-closed guards. Removing them would silently accept a stale phone-home config, which is a security regression. The cost is a few lines. |
| K-3 | **`--transport=http` accepted and validated** | `internal/cli/cli.go:552-561` | The npm launcher passes it on every start. |
| K-4 | **Standalone rejection of platform settings** (`OPUTE_MCP_URL` and `OPUTE_MCP_HEALTH_URL` in standalone mode) | `internal/config/config.go:285-291` | A mode-isolation guard. |
| K-5 | **Recipe `compatibility.minHostAgentVersion` / `requiredTools`** | `internal/recipe/recipe.go:40-70`, `internal/hostmcp/*_run.go` | This is live recipe admission, not legacy. **Caution:** Rust must report a version that compares exactly as Go's does (see M10 `--version`). |
| K-6 | **`probe_openai_compatible_server`** | `internal/domain/llm/runtime_probe.go` | "Compatible" here means the OpenAI API shape. It is a current tool. |

## 7. Summary

| Class | Count | Effect on the port |
| --- | --- | --- |
| I (drop now) | 26 | Removes the biggest structural costs: six descriptor sources become one, the adapter layer goes away, the global registry and the env-as-config-bus go away, and ad-hoc migrations become versioned. No contract change. |
| O (separate retirement) | 21 | If retired in Go first (Option B), Rust skips roughly a dozen tools, 14 alias operations, and about ten argument and default shims. |
| K (keep) | 6 | No change |
| G (apparent Go defect) | 1 | Reproduce it, or fix it in both implementations with evidence |

## 8. Follow-ups this inventory creates

1. **M0:** fold every I, O, and K item into `baseline/inventory/` with an
   owner and a parity scenario. An I item without a guard scenario cannot be
   dropped.
2. **Separate OpenSpec change** (proposed name
   `retire-host-agent-compat-surface`, against the Go repository and Platform):
   gather call-log evidence for O-1…O-17 and retire what has zero use. Then
   rebase `baseline/source-lock.json`.
3. **Owner decisions:** O-19 (Postgres ownership), O-20 (profile model), and
   O-21 (Windows), plus a reproduce-or-fix decision on G-1, join D1–D6 in [milestones.md](milestones.md#6-decisions-needed-from-owners-before-or-during-implementation).
