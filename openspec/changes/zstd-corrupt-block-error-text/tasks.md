## 1. Decision

- [x] 1.1 Obtain the owner's decision between porting klauspost's decoder, mapping common messages, and declaring the corrupt-block text. Decided 2026-10-12 as D20: declare it narrowly.
- [x] 1.2 Add D20 to `parity-manifest.json` decisions and to the migration's owner decision table.

## 2. Evidence

- [x] 2.1 Add the `keepPrefix` divergence operation, with a harness test showing that a text without the prefix stays a diff.
- [x] 2.2 Declare `D20.corrupt-zstd-block` and add `host-domain.archives.corrupt-blocks`, with per-side expectations. Go/Go 3/3, Go/Rust 2/2.
- [x] 2.3 Include the case in the M8a closing captures: Go/Go x20 and Go/Rust x5. Round 7 (`evidence/m8a`): pass in both suites.
