## 1. Decision

- [x] 1.1 Owner answers Q17 (rust-spec-authority/questions.md).

## 2. Implementation (only after 1.1)

- [x] 2.1 Failing-first test: after an acknowledged reset, a left-out NotReady server is retired, a still-Ready one is refused as not settled, and without a reset the majority proof stands (`removal_after_recovery_waits_for_it_to_settle`).
- [ ] 2.2 Rerun e2e3a.quorum-removal with retirement as a pass condition.
