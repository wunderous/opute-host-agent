## 1. Decision

- [x] 1.1 Decide S2 and S6 (delegated by the owner, 2026-11-11): D48.

## 2. Implementation and evidence

- [x] 2.1 The fix on main, merged into rust-0.4.0.
- [x] 2.2 Failing-first test `providers::cloudflare::tests::a_host_connector_never_carries_its_run_token_on_argv and an_ha_connector_keeps_its_run_token_in_a_private_file`.
- [x] 2.3 The dev VM cannot reach this path; the unit test is its check.
