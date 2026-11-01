## 1. Decision

- [x] 1.1 Obtain the owner's decision on the M8c names. Decided 2026-10-16 as D22: the nine renames in the proposal, with Go's names kept as hidden aliases.
- [x] 1.2 Add D22 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Add the entries to `crates/host-agent/catalog/neutral-names.json`.
- [x] 2.2 Re-pin every Rust catalog revision with its Go pair, and update `catalog::tests::revisions_match_manifest`. Done 2026-10-16: platform and standalone in the crate test, and all fourteen provider-fixture pairs, each observed one-to-one in a Go/Rust run (the rival catalog in M5 shape).

## 3. Evidence

- [x] 3.1 Alias scenario: each Go name reaches the same Consumer as its neutral name and is not listed; the four unpublished Go tools are refused as Go refuses them. Done in T1: every cluster-domain scenario calls the Go names (D19 compares tools/list), and `cluster-domain.unreachable` refuses the four.
- [ ] 3.2 Include the rename in the M8c closing captures: Go/Go x3 and Go/Rust x1 (repeat rule, 2026-11-01).
