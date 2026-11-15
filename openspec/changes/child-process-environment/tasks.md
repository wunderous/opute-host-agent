## 1. Decision

- [x] 1.1 Decide A4 (delegated by the owner, 2026-11-11): an allowlist for every child, D40.
- [x] 1.2 Add D40 to the migration's owner decision table; record A4 in the auth review.

## 2. Implementation and evidence

- [x] 2.1 `Env::for_child`; every site clears the inherited environment and passes it.
- [x] 2.2 Failing-first tests: the spawn-site source test and the `env` test (both failed on rc1's code, `evidence/m17/before.log`).
- [x] 2.3 A property: no generated environment's secret-named variable survives `for_child`; the entry joins `evidence/hardening/entry-points.json`.
- [x] 2.4 Gate e2e-6 reads a running console's and a recipe step's environment from `/proc` on the release candidate. Passed: `e2e6.child-env`, run `m17-r2` on `rust-0.4.0-rc2`.

## 3. The 0.3.0 candidate (D43, goal m18)

- [x] 3.1 The same allowlist at 0.3.0's seven sites; the source test counts seven.
- [x] 3.2 The tests failed on rust-0.3.0-rc4's code (`evidence/m18/before.log`).
- [ ] 3.3 Gate e2e-6 reads a recipe step's environment from `/proc` on rust-0.3.0-rc5 (`e2e6.child-env`).
