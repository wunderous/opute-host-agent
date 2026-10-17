## 1. Decision

- [x] 1.1 Obtain the owner's decision on the M8d naming. Decided 2026-10-17 as D23: neutral selector values for `builder` and `runtime`, with Go's values kept as hidden aliases (`evidence/m8d/NOTE.md`).
- [x] 1.2 Add D23 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Add the value section to `crates/host-agent/catalog/neutral-names.json`. Publish the mapped enums, accept Go's values in calls and plan nodes, and publish mapped result values.
- [x] 2.2 Re-pin every Rust catalog revision with its Go pair, and update `catalog::tests::revisions_match_manifest`. Done 2026-10-17: platform and standalone in the crate test, and all fourteen provider-fixture pairs, each observed one-to-one in a Go/Rust run (the rival catalog in M5 shape). Go/Rust ×1: 287/287.
- [x] 2.3 Extend `D19.neutral-host-tool-names` in `tools/parity/parity/canon.py` to rewrite the mapped values on the Rust side.

## 3. Evidence

- [ ] 3.1 Value scenario: each Go value reaches the same Consumer as its published value, and results publish the mapped value. The OCI T1 scenarios call with Go's values, and D19 compares `tools/list` and results.
- [ ] 3.2 Include the value map in the M8d closing captures: Go/Go ×20 and Go/Rust ×5.
