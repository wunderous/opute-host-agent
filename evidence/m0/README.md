# M0 evidence: parity harness, inventory and fail-closed verifier

This file records the M0 results from
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md)
as they stood when M0 merged. The evidence bundles themselves now live in
[`evidence/current/`](../current/), which is regenerated whenever the harness
changes (a harness change invalidates older bundles); see
[evidence/m1/README.md](../m1/README.md) for the latest numbers.
To reproduce it from a clean checkout, run `make parity-m0` (Go 1.24.7,
Python 3.11). Provenance is bound to `baseline/source-lock.json`: Go commit
`ace7013d…`, tree `ac17cfb2…`, and Go reference binary sha256 `71924b62…`,
which is reproducible across checkout directories.

## Gate results

| Gate | Result | Why |
| --- | --- | --- |
| `make parity-verify` (M0) | **PASS** | Go vs Go 19/19 scenarios clean on all 20 iterations; 6/6 canaries caught; inventory hashes match; 0 unowned gaps; 19/19 Rust items present and `unverified` |
| `make parity-verify-cutover` | **FAIL (by design)** | No Rust candidate exists, so all 19 go-vs-rust items are `unverified` |
| `make parity-test` | 29/29 | 10 comparator tests, 19 verifier negative tests (§2.5.3) |

## What the harness proved about itself (§2.5)

1. **Determinism.** Every scenario ran 20 times with the pinned Go binary on
   both sides, with distinct IDs, ports, tokens and state: 760 twin
   comparisons, 0 diffs. Two scenarios failed on the first run and were
   fixed by understanding them, not by masking them:
   - The ADR 0012 tool-name prefix is derived from the agent ID. The harness
     now derives the expected prefix itself (UUIDv5), so a Rust derivation
     error would show up as a diff.
   - `get_host_info.supportedTools` order changes between runs (finding F-2),
     so it is declared a set with a note. Live host measurements (memory,
     disk, load, PSI, `checkedAt`) use typed masks. Each mask checks the
     value's type and states its reason.
2. **Canaries.** Six patched Go builds each turn their named scenario red and
   nothing unexpected: descriptor text (C1), 401 → 403 (C2), one extra
   `incus` argv token (C3), the ADR 0011 bypass list gaining `tasks/get`
   (C4), a retired phone-home variable being accepted (C5), and the
   standalone mutation gate opened (C6). The first drafts of C1 and C2 were
   *not* caught because they patched code that never reaches the wire. That
   is recorded in `canaries.json`, and it produced finding F-6.
3. **Verifier negative tests.** Missing, modified, stale (source lock, Go
   binary, harness, scenario, repeat count), blocked and malformed evidence
   all fail the gate. So do an uncaught or missing canary, a tampered
   inventory and an unowned scenario. A summary's own `status` field is
   never trusted.

## Inventory (`baseline/inventory/`)

| File | Content |
| --- | --- |
| `catalog/{standalone,standalone-mutations,standalone-prefixed,platform}.json` | Live authenticated `tools/list`: 142 / 142 / 142 / 130 descriptors with effect, approval, provider, resourceCost, and a per-descriptor hash |
| `dispatch-registry.json` | 119 dispatch names: 108 published, 6 excluded by design, 5 in no catalog (F-5) |
| `env.json` | 69 environment variables with source anchors; 11 covered by a scenario |
| `http.json` | 8 routes; 6 covered (`/oauth/authorize` and `/oauth/revoke` are gaps) |
| `state.json` | Tables found statically, plus the live schema of `state/state.db` and `state/authz.sqlite` after start |
| `effects.json` | 17 external commands; only `incus` is observed through shims so far |
| `plans.json` | 18 recipes with hashes and live `recipe validate` output |
| `distribution.json` | Release artifact names, npm package fields, deploy unit hashes |
| `findings.json` | Findings G-1 and F-2 to F-6 (below) |
| `gaps.json` | 101 explicit gaps, every one owned |

Capture is deterministic: two captures against the same source and binary
are byte-identical. Where Go itself is nondeterministic (F-4), every
observed variant is recorded.

## Findings

| ID | Finding | Decision needed |
| --- | --- | --- |
| G-1 | Confirmed live: every local-LLM tool is published as "LEGACY compatibility wrapper … prefer runtime-recipe.v1" with an untyped output. That includes `ensure_local_llm_relay`, the path the k3s proxy tool tells callers to prefer | Reproduce in Rust, or fix in both |
| F-2 | `get_host_info.supportedTools` order is random (map iteration) | None: compared as a set |
| F-3 | `OPUTE_TRANSPORT=stdio` is reported as `invalid --transport "stdio"` | Reproduce the text exactly |
| F-4 | `recipe validate` names a random missing required input (Ollama recipes) | Rust emits one of the same variants, or fix in both |
| F-5 | `ensure_k3d`, `get_cluster_details`, `get_cluster_runtime_details`, `list_clusters` and `uninstall_helm_chart` are dispatch-registered but appear in no catalog | Check whether `tools/call` reaches them (a hidden surface) before M3 |
| F-6 | The published `apply_manifest` text comes only from `schemas/incus-tools.json`. The copies in `catalog.go`, `all-tools.json` and `standalone.go` do not reach the wire | Extend the check to every descriptor before M3 (supports legacy-inventory I-1/I-2) |

Also observed: a fresh state directory still creates the legacy
`active_runtimes` table next to `active_capabilities` (relevant to
legacy-inventory I-23 and O-17).

## What M0 does not yet cover (explicit gaps, owned)

These do not block the M0 gate, which requires owned gaps rather than zero
gaps. They are what later milestones pick up:

- **Provider catalog cells.** Catalogs with k3s, Cloudflare, Tailscale,
  Ollama or hostos active need provider install fixtures (M7). Only the
  no-provider cells are captured.
- **Env and effects coverage.** 58 variables and 16 commands have no scenario
  yet. Commands such as `systemctl`, `apt-get` and `podman` will be observed
  as M8 domain scenarios drive them.
- **OAuth flows.** `/oauth/authorize`, token exchange and revoke (M2).
- **Plans.** Execution parity (M6). Only validation output is inventoried.
- **Remaining canaries from §2.5.** An unredacted secret needs secret-bearing
  state scenarios (M5), and a reordered plan node state needs plan execution
  (M6). They are added when those scenarios exist.
- **Task 0.5 owner review.** The verifier exists and fails closed, but the
  invariant in `.agents/decisions/rust-cutover-parity-gate.json` still needs
  review by the Host Agent and Platform owners, so it stays `proposed`.
