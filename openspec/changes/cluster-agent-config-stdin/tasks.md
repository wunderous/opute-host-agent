## 1. Decision

- [x] 1.1 Decide S4 (delegated by the owner, 2026-11-11): D50.

## 2. Implementation and evidence

- [x] 2.1 The fix on main, merged into rust-0.4.0.
- [x] 2.2 Failing-first test `clusterops::tests::the_bridge_token_never_travels_on_argv`.
- [x] 2.3 Gate e2e-6 on rust-0.3.0-rc6 and rust-0.4.0-rc4: the journey sees no bridge token on any argv or in the journal, and the config is 0600 (`e2e6.secret-sweep`, runs `m19-r1` and `m19-r2`).
