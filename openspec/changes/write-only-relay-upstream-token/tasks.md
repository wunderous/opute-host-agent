## 1. Decision

- [x] 1.1 Decide S5 (delegated by the owner, 2026-11-11): D51.

## 2. Implementation and evidence

- [x] 2.1 The fix on main, merged into rust-0.4.0.
- [x] 2.2 Failing-first test `catalog::tests::every_declared_credential_input_is_write_only`.
- [x] 2.3 Gate e2e-6 on rust-0.3.0-rc6 and rust-0.4.0-rc4: the journey finds no upstream token in the store (`e2e6.secret-sweep`, runs `m19-r1` and `m19-r2`).
