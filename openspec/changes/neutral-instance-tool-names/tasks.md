## 1. Decision

- [x] 1.1 Obtain the owner's decision on the M8b names. Decided 2026-10-14 as D21: `reset_incus_stack` becomes `reset_virtualization_stack`, and `provision_container.wslGpuLibs` becomes `hostGpuLibs`, with Go's names kept as hidden aliases.
- [x] 1.2 Add D21 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Add the entries to `crates/host-agent/catalog/neutral-names.json`, with the field rename on a tool that keeps its own name.
- [x] 2.2 Re-pin every Rust catalog revision with its Go pair, and update `catalog::tests::revisions_match_manifest`.

## 3. Evidence

- [ ] 3.1 Extend the alias scenario: `reset_incus_stack` and `wslGpuLibs` reach the same Consumer as their neutral names, and neither is listed.
- [ ] 3.2 Include the rename in the M8b closing captures: Go/Go x20 and Go/Rust x5.
