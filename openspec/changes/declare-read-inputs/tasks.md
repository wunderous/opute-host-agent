## 1. Decision

- [x] 1.1 Owner answers Q21 (rust-spec-authority/questions.md).

## 2. Implementation (only after 1.1)

- [x] 2.1 Failing-first catalog test: every input a handler reads is declared.
- [x] 2.2 Declare serviceName and ipv4Address (or derive serviceName from uri); repin the catalog revision.
- [x] 2.3 Rerun e2e2.host-ops and e2e2.incus with declared inputs only. Passed in run `e2e-13a-r6`.
