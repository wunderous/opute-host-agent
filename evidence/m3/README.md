# M3 evidence: catalog publication and read-only host tools

This file records M3 from
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md)
(tasks 1.2 and the read-only part of 2.3). The evidence is in
[`evidence/current/`](../current/). Reproduce it with `make parity-m3`
(Rust 1.94.1, Go 1.24.7 with the go1.25.4 toolchain the reference binary was
built with, Python 3.11).

## Gate results

| Check | Result |
| --- | --- |
| `make rust-check` | fmt, clippy `-D warnings`, 73 unit tests: pass |
| `make parity-test` | 71 harness tests (comparator, omitempty, one-of masks, substitution, divergences, verifier, contracts, corpus freshness): pass |
| `catalog-source --check` (in `make parity-ci`) | the committed `crates/host-agent/catalog/source.json` equals a fresh export from the pinned Go tree |
| Go vs Go, ×20 | 66/66 scenarios clean on all 20 iterations |
| Canaries | 10/10 caught, each by its named scenario (plus declared co-failures); C9 and C10 are new |
| **Go vs Rust, ×5** | **66/66 scenarios pass on all 5 iterations, on every surface** (admission, auth, catalog, cli, config, durable-state, host-read, http, lifecycle, mcp-wire, read-only-tools), outside the declared D8 divergences |
| Go oracle tests against Rust | `standalone-startup` (M1), `real-client-modern` (M2) and `real-client-catalog` (M3, the go-sdk client lists the catalog and calls read-only tools) pass against Go and Rust and fail against the `/bin/true` control |
| Declared-divergence contract suites and Rust canaries | D8 `oauth-issuance` 24/24, D10 `standalone-read-only-gate` 8/8; 20/20 Rust canaries caught ([D8](../oauth-issuance/README.md), [D10](../standalone-gate/README.md)) |
| `make parity-verify-m1` … `parity-verify-m3` | **PASS** |
| `make parity-verify-cutover` | **FAIL, by design**: 80 owned inventory gaps (no covering scenario yet) remain; see below |

## One catalog source

```
 pinned Go tree ──► tools/catalog-gen (overlay, runs Go's own loaders)
                          │  declarations only: definitions, registrations,
                          │  internal definitions, residual effects, task-aware set
                          ▼
            crates/host-agent/catalog/source.json   (committed, generated)
                          │
                          ▼  catalog.rs derives, exactly like Go:
        descriptors · edges · admission metadata · revision (sha256 of Go json bytes)
                          │
          ┌───────────────┼──────────────────────┐
          ▼               ▼                      ▼
     tools/list     schema.rs validation    get_capability_catalog
```

- The generator exports declarations only. Derived values (descriptors,
  dependency edges, the revision) are computed by Rust, so a derivation bug
  shows up as a diff instead of being copied from Go.
- The revision is SHA-256 over the bytes Go's `json.Marshal` produces, so
  `gojson` reproduces HTML escaping, `\b`/`\f`, Go float formatting, sorted
  map keys, struct field order and `omitempty`.
- Go builds some lists by ranging over maps; the export sorts each list and
  the driver rejects repeated names, so generation is deterministic.
- Drift fails closed in two places: `parity-ci` runs
  `catalog-source --check` against the pinned tree, and the crate test
  `catalog::tests::revisions_match_manifest` pins both revisions to the
  reviewed values in `parity-manifest.json` (X5):

| Mode | Tools | Revision |
| --- | --- | --- |
| standalone | 130 | `sha256:50b20e5885a225bd45f0a67d2c0327dfa7d096f845c71b81ca689d37c153f6a2` |
| platform | 142 | `sha256:7f300d1de913066fe8812e21f5f926c8548e607ec9d3d65d54300377059c07ec` |

The four catalog cells (standalone, standalone with mutations allowed,
platform, prefixed names) are byte-identical between Go and Rust: names,
descriptors, schemas, edges and revision.

## What the Rust agent does at M3

`tools/call` follows Go's pipeline step by step:

```
 unknown name ─► -32602 (SDK)
 prefix rewrite (OPUTE_TOOL_PREFIX) ─► catalog revision check (catalog_revision_stale)
 argument decode ─► standalone mutation gate ─► lifecycle tools
 task-aware tool without the tasks extension ─► -32003 + requiredCapabilities
 dispatch: input schema (invalid_arguments) ─► handler ─► output schema (invalid_result)
```

| Module | Scope |
| --- | --- |
| `catalog` | Source loading, descriptor and edge derivation, revision, published tool list per mode and prefix |
| `schema` | A port of `plan.ValidateJSON` with Go's error texts (`%T` names, strconv `'g'` floats) |
| `tools` | The call pipeline, result helpers (struct-ordered text content), and the read-only handlers |
| `incus` | Provider binary resolution, Go-typed list decoding, ownership enforcement, VM mapping, root-disk quota |
| `resource` | Canonical resource URIs, registration, and the coordinator's capacity snapshot |
| `hostobs` | Pressure stall, cgroup and disk statistics, enforcement observation, platform detection |

Read-only tools served: `get_capability_catalog`, `get_host_info`,
`get_host_capacity`, `list_vms`, `detect_host_platform`. Every other tool is
published unchanged and fails closed with a typed `not_implemented`
capability error until its milestone.

## Scenarios

`scenarios/host-read.json` (22 scenarios) runs each tool against Incus shims:

| Family | Variants |
| --- | --- |
| `list-vms` | mixed inventory, two containers, empty, malformed output, failing list, Incus absent, ownership enforced, platform mode |
| `host-info` | mixed inventory, ZFS storage, Incus absent, failing list, platform mode |
| `detect-platform`, `host-capacity` | native host |
| `capability-catalog` | standalone and platform |
| `call-envelope` | argument decode and schema errors |
| `task-boundary.platform` | task-aware tools without the tasks extension (X2) |
| `lifecycle-without-tasks.platform` | lifecycle tools without the tasks extension |
| `standalone-mutation-set-denied` | every one of the 71 gated tools, plain and prefixed (X2) |

**X2** (rejected calls leave no trace): scenarios that declare
`"invariants": ["X2"]` fail on either side if a rejected call left a task,
reservation, resource or invocation row behind.

New canaries: **C9** (`vm-release-default`) and **C10**
(`schema-type-text`) mutate the Go reference. Each is caught by
`host-read.list-vms.inventory-mix`.

## What validation caught (and what changed)

1. **Go validates arguments before the handler.** The first Rust handlers
   accepted any arguments; Go answers `invalid_arguments` with exact
   `ValidateJSON` texts, so `schema.rs` is a port, including the
   `vmName` → `name` alias.
2. **Text content is struct-ordered JSON.** Go's text content marshals a
   struct (declaration order), not a map (sorted). Rust encodes it through a
   typed `Shape` so the field order matches.
3. **Prefixed names are rewritten in the request.** With a tool prefix, Go
   rewrites the incoming name (body and `Mcp-Name` header) before dispatch.
4. **The `-32003` data carries `roots`.** The required-capabilities data
   includes an empty `roots` object next to the tasks extension.
5. **PSI fields are `omitempty`.** Pressure-stall values that are zero are
   absent on some runs of Go itself; the comparator now applies declared
   `compare.omitempty` rules (each with a reason).
6. **Port substitution inside numbers.** The harness replaced a port's digits
   inside an unrelated number; substitution now respects digit boundaries.
7. **The cutover gate was not fail closed.** With every existing scenario
   green, `parity-verify-cutover` passed, although the inventory still lists
   80 owned Go behaviours that no scenario covers yet. Before M3 a
   failing scenario hid this. The cutover gate now also requires
   `requireNoGaps`: zero inventory gaps, owned or not. Verifier tests cover
   the gaps case and a missing gap count.
8. **A racing disk mount.** `get_host_info` reports the default disk path
   (`/` or `$HOME`) with the fewest available bytes. In the sandbox both
   are the same filesystem, so Go and Rust alike pick either one, depending
   on concurrent writes (2 of 96 stressed runs). A `one-of` mask accepts
   exactly the declared candidates, and 144 stressed runs are clean.
9. **Nondeterministic export.** Go appends some definitions in map order; the
   export sorts them.

## Scope changes and open items

- **Moved to M4:** resource-bound reads (`get_vm_info` and every tool with a
  declared resource argument), `normal`-class reads that Go's admission
  refuses while workload enforcement is unverified (`inspect_host_file`,
  `probe_http_endpoint`, …), reservations, typed admission and tenant scope
  (rest of task 2.3).
- **Moved to M5:** durable invocation evidence (`capability_invocations`),
  which needs schema-derived redaction. M3 compares results, typed errors
  and command traces instead.
- **Domain reads** (Kubernetes, PostgreSQL, OCI, LLM, recipes, plans,
  operations) arrive with their domains.
- **Provider catalog cells** (installed providers) stay the owned inventory
  gap for M7.
- **Real hosts (T2):** Ubuntu x64/arm64 and WSL2 runs are not available in
  this environment. `get_host_info` and `detect_host_platform` are compared
  live on the CI host only; WSL detection is covered by unit tests, because
  the agent refuses to start with WSL markers in a fixture environment.
- **Decision D10 (standalone read-only gate).** After M3 the owner approved
  a Rust-only gate: with standalone mutations disabled, Rust runs only tools
  whose effect is `read`. It is specified in
  [`standalone-read-only-gate`](../../openspec/changes/standalone-read-only-gate/proposal.md)
  and verified by its own contract suite and canaries
  ([evidence](../standalone-gate/README.md)). The `m3` and cutover gates
  require that suite. Go-vs-Rust comparisons are unaffected: no twin scenario
  calls a tool that the two gates treat differently.
- **Declared D8 divergences** now also cover
  `admission.standalone-mutation-denied` and `state.schema-after-start`,
  whose state schema includes the D8 credential tables.
