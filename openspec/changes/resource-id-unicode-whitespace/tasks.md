## 1. Decision

- [x] 1.1 Owner decision between refusing such ids and recording the finding. Decided 2026-11-10 as D30: refuse them.
- [x] 1.2 Add D30 to the migration's owner decision table.

## 2. Implementation and evidence

- [x] 2.1 `Uri::new` and `Uri::parse` refuse an id with any Unicode whitespace.
- [x] 2.2 Failing-first test with U+000B, U+000C, U+0085, U+2003 and U+3000; the round-trip property runs without excluding them.
