## 1. Decision

- [x] 1.1 Obtain the owner's decision on the M8c names. Decided 2026-10-17 as D22: the nine renames in the proposal, with Go's names kept as hidden aliases.
- [x] 1.2 Add D22 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Add the entries to `crates/host-agent/catalog/neutral-names.json`.
- [ ] 2.2 Re-pin every Rust catalog revision with its Go pair, and update `catalog::tests::revisions_match_manifest`.

## 3. Evidence

- [ ] 3.1 Alias scenario: each Go name reaches the same Consumer as its neutral name and is not listed; the four unpublished Go tools are refused as Go refuses them.
- [ ] 3.2 Include the rename in the M8c closing captures: Go/Go x20 and Go/Rust x5.
