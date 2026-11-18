## 1. Decision

- [x] 1.1 Decide the placement rules (delegated by the owner, 2026-11-11): D46.
- [x] 1.2 Add D46 to D51 to the migration's owner decision table and the packet.

## 2. Measurement

- [x] 2.1 Inventory every secret class and its paths (`evidence/m19/inventory.md`).
- [x] 2.2 `e2e6.secret-sweep` with its guest tool, and the harness's `excludes` assertion.
- [x] 2.3 Run it on rust-0.3.0-rc5 (`m19-before-030`) and rust-0.4.0-rc3 (`m19-before-040`): S1, S4 and S5 on both.
- [x] 2.4 Unit tests for the paths the dev VM cannot reach (S2, S3, S6), failing on rc5's and rc3's code (`evidence/m19/before.log`, `before-040.log`, `before-s6.log`).

## 3. Candidates

- [ ] 3.1 Gate e2e-6 passes `e2e6.secret-sweep` on rust-0.3.0-rc6.
- [ ] 3.2 Gate e2e-6 passes `e2e6.secret-sweep` on rust-0.4.0-rc4.
