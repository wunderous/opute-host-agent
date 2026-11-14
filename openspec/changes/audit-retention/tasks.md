## 1. Decision

- [x] 1.1 Decide O1 (delegated by the owner, 2026-11-11): retention by age and rows, D42.
- [x] 1.2 Add D42 to the migration's owner decision table; record O1 closed in the hardening evidence.

## 2. Implementation and evidence

- [x] 2.1 `StateStore::prune_audit`, the two settings with their defaults, and the background pruner.
- [x] 2.2 Failing-first test (failed on rc1's behaviour, `evidence/m17/before.log`).
- [x] 2.3 The state store's entry in `evidence/hardening/entry-points.json` covers pruning.
- [x] 2.4 `e2e6.budget` shows the store bounded under the 15-minute load on the release candidate. Passed: `e2e6.budget`, run `m17-r2` on `rust-0.4.0-rc2`.
