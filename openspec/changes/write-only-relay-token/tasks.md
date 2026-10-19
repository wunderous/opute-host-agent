## 1. Decision

- [x] 1.1 Obtain the owner's decision on the relay token Go stores verbatim. Decided 2026-10-19 as D24: mark it `writeOnly` (`evidence/m8e/NOTE.md`).
- [x] 1.2 Add D24 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Add `crates/host-agent/catalog/write-only.json` and mark the listed paths in the published definitions. The crate test `d24_marks_the_relay_token_write_only` checks each mark and the storage redaction, and that Go's definitions stay unmarked.
- [x] 2.2 Re-pin every Rust catalog revision with its Go pair: platform and standalone in the crate test, and each provider-fixture pair observed one to one in a Go/Rust run. Done 2026-10-19: all fourteen fixture pairs (the rival catalog in M5 shape). Go/Rust ×1: 314/314; M5 shape 311/311 after the rival pin.
- [x] 2.3 Extend `D19.neutral-host-tool-names` to remove exactly the listed marks on the Rust side.

## 3. Evidence

- [x] 3.1 Extend the M5 secret sweep: nested fields, and the D24 fields swept on Rust only, with Rust's catalog checked to mark each. Pre-round: 16 Go fields and 8 D24 fields, 0 failures.
- [ ] 3.2 Include D24 in the M8e closing captures: Go/Go ×20, Go/Rust ×5, the secret sweep and M5 shape.
