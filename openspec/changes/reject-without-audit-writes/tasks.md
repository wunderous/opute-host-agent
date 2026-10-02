# Tasks

- [x] Record owner-approved D11 in the parity manifest and milestone decisions.
- [x] Compare the rejection wire behavior against pinned Go with only the invocation count declared divergent.
- [x] Run a Rust contract that checks all database row contents and command traces before and after each rejection, post-rejection reads and restart.
- [x] Prove the contract detects an injected rejected-call audit write with a Rust canary.
- [x] Refresh differential, contract, oracle, canary and verifier evidence; keep whole-agent cutover closed for unrelated inventory gaps.

Evidence: [local E2E report](../../../evidence/local-e2e/README.md),
[M3 gate](../../../evidence/current/verify-m3.json), and
[cutover gate](../../../evidence/current/verify-cutover.json).
