# Tasks

- [x] Record owner-approved D12 in the parity manifest and milestone decisions.
- [x] Compare every task-aware admission refusal against pinned Go, declaring only the Go operation-row count as divergent (`admission.matrix.binding`, `admission.matrix.unknown-target`).
- [x] Run a Rust contract that checks all database rows and command traces before and after each refusal, the task's wire outcome, post-refusal reads and restart.
- [x] Prove the contract detects an injected operation row on the refusal path with a Rust canary.
- [ ] Keep the boundary when durable operation records arrive (M5): persist only admitted calls.

Evidence: [M4 report](../../../evidence/m4/README.md).
