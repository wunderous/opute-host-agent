## 1. Decision

- [x] 1.1 Decide U1 (delegated by the owner, 2026-11-11): D52.

## 2. Implementation and evidence

- [x] 2.1 The fix on main; `rust-0.4.0` keeps its N4 probe.
- [x] 2.2 Failing-first test `hostpublic::tests::the_origin_probe_uses_an_in_process_token` (`evidence/m21/before-d52.log`).
- [x] 2.3 `e2e4.upgrade-chain` on rust-0.3.0-rc7 and rust-0.4.0-rc4 (run `m21-r2`): each 0.3.0 connector call gets past its origin probe, before the upgrade and after the rollback.
