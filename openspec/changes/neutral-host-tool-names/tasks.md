## 1. Decision

- [x] 1.1 Obtain the owner's decision on the publication mode and scope. Decided 2026-10-12 as D19: neutral names with hidden, deprecated Go aliases, covering tool names, provider-named fields, and `ensure_docker`/`ensure_k3d`.
- [x] 1.2 Add D19 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [ ] 2.1 Define the name map once in the descriptor source, and derive from it the published descriptors, the alias table and the field rename.
- [ ] 2.2 Accept aliases in `tools/call` with Go's input schema, routing to the same Consumer and recording the called name.
- [ ] 2.3 Pin the new Rust catalog revisions per mode, each with its Go pair, and update `catalog::tests::revisions_match_manifest`.

## 3. Evidence

- [ ] 3.1 Implement and test the `D19.neutral-host-tool-names` divergence. A canary must show that an unmapped rename and an unpinned revision both stay diffs.
- [ ] 3.2 Rerun the M1-M7 suites Go/Rust with the divergence applied and not stale, and Go/Go unchanged.
- [ ] 3.3 Add alias scenarios: a Go-name call with `incusPackage` matches Go, and the call is absent from `tools/list`.
