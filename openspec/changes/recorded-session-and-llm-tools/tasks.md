## 1. Implementation

- [x] 1.1 Built-in services for the three tools on the D14 seam; the seam verifier reports them compliant.
- [x] 1.2 Unit tests for each outcome, failing first.

## 2. Evidence

- [x] 2.1 `e2e3.serving` calls both LLM tools, and a contract test covers `open_assistant_session`, on the 0.4.0 candidate. Passed: `e2e3.serving`, run `m15-r1` on `rust-0.4.0-rc1`; `seam::tests::recorded_tools_answer_as_recorded` covers `open_assistant_session`.
