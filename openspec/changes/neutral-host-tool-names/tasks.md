## 1. Decision

- [x] 1.1 Obtain the owner's decision on the publication mode and scope. Decided 2026-10-12 as D19: neutral names with hidden, deprecated Go aliases, covering tool names, provider-named fields, and `ensure_docker`/`ensure_k3d`.
- [x] 1.2 Add D19 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Define the name map once in the descriptor source, and derive from it the published descriptors, the alias table and the field rename. `crates/host-agent/catalog/neutral-names.json`; `catalog.rs` renames definitions, registrations and task-aware names when the source loads, and builds the alias descriptors from Go's own definitions.
- [x] 2.2 Accept aliases in `tools/call` with Go's input schema, routing to the same Consumer and recording the called name. Every name lookup goes through `Snapshot::find`; `builtin::invoke` moves `incusPackage` to `packageName` before the Consumer reads it; provider callbacks may name an alias.
- [x] 2.3 Pin the new Rust catalog revisions per mode, each with its Go pair, and update `catalog::tests::revisions_match_manifest`. The test also proves each Go value is the un-renamed catalog's revision. The 14 provider-fixture catalog pairs are pinned under `providerFixtures`.

## 3. Evidence

- [x] 3.1 Implement and test the `D19.neutral-host-tool-names` divergence. A canary must show that an unmapped rename and an unpinned revision both stay diffs. `renameToGo` in `tools/parity/parity/canon.py`; `tests/test_canon.py::NeutralNamesTest` holds the canary.
- [x] 3.2 Rerun the M1-M7 suites Go/Rust with the divergence applied and not stale, and Go/Go unchanged. 2026-10-12, all 256 scenarios: Go/Rust 256/256 with D19 declared in the 93 scenarios that observe it (none stale); Go/Go 256/256. M8a's closing evidence repeats these captures.
- [x] 3.3 Add alias scenarios: a Go-name call with `incusPackage` matches Go, and the call is absent from `tools/list`. `host-domain.d19.aliases`: Go/Go x2 and Go/Rust pass; the package reaches apt on both sides, an unmapped near-name is unknown on both, and the audit rows name the alias.
