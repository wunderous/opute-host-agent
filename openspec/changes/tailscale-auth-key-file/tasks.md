## 1. Decision

- [x] 1.1 Decide S3 (delegated by the owner, 2026-11-11): D49.

## 2. Implementation and evidence

- [x] 2.1 The fix on main, merged into rust-0.4.0.
- [x] 2.2 Failing-first test `providers::tailscale::tests::the_auth_key_never_reaches_tailscales_arguments`.
- [x] 2.3 The dev VM cannot reach this path; the unit test is its check.
