## 1. Decision

- [x] 1.1 Decide A4 (delegated by the owner, 2026-11-11): an allowlist for every child, D40.
- [x] 1.2 Add D40 to the migration's owner decision table; record A4 in the auth review.

## 2. Implementation and evidence

- [x] 2.1 `Env::for_child`; every site clears the inherited environment and passes it.
- [x] 2.2 Failing-first tests: the spawn-site source test and the `env` test (both failed on rc1's code, `evidence/m17/before.log`).
- [x] 2.3 A property: no generated environment's secret-named variable survives `for_child`; the entry joins `evidence/hardening/entry-points.json`.
- [ ] 2.4 Gate e2e-6 reads a running console's and a recipe step's environment from `/proc` on the release candidate.
