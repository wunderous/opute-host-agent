## 1. Authority

- [x] 1.1 Reword the contract requirements so they cite the frozen contract record and the accepted decisions, not Go.
- [x] 1.2 Replace the parity-based cutover requirement with a specification-based one.
- [x] 1.3 Record each behaviour that only Go decided as an owner question (questions.md).
- [x] 1.4 Update AGENTS.md: the spec and the design docs are the authority, and Go is a migration source only.
- [x] 1.5 Amend `.agents/decisions/rust-cutover-parity-gate.json` (D27).

## 2. CI and evidence

- [x] 2.1 Replace the CI Go/Rust parity job with the Rust-only suites (`make rust-suites`).
- [x] 2.2 Mark the committed parity evidence as historical in milestones.md, and make M13 specification-based.

## 3. Archive

- [x] 3.1 After the owner answers questions.md (D28, 2026-11-07), archive this change and rewrite the spec's Purpose so it no longer names Go.
