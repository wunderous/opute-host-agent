## 1. Decision

- [x] 1.1 Decide P1 (Host Agent owner, 2026-11-21): D53, loopback only.

## 2. Implementation and evidence

- [ ] 2.1 Land the fix on main, then merge it into `rust-0.4.0`.
- [x] 2.2 Failing-first test `providers::tests::a_provider_listens_on_loopback_only` (`evidence/m25/before-d53.log`).
- [ ] 2.3 Every journey that installs a provider shows its socket on `127.0.0.1` only, on rust-0.3.0-rc8 and rust-0.4.0-rc5.
