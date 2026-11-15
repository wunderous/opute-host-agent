## 1. Decision

- [x] 1.1 Decide A2 and A5 (delegated by the owner, 2026-11-11): bound both reads, D41.
- [x] 1.2 Add D41 to the migration's owner decision table; record A2 closed and A5 in the auth review.

## 2. Implementation and evidence

- [x] 2.1 The relay reads at most 32 MiB; a provider's `/mcp` reads at most 4 MiB; each answers 413 beyond.
- [x] 2.2 Failing-first tests for both (failed on rc1's code, `evidence/m17/before.log`).
- [x] 2.3 Both reads keep their coverage in `evidence/hardening/entry-points.json`.
- [x] 2.4 Gate e2e-6 sends an oversized relay body on the release candidate: 413, with RSS flat. Passed: `e2e6.pressure`, run `m17-r2` on `rust-0.4.0-rc2`.

## 3. The 0.3.0 candidate (D43, goal m18)

- [x] 3.1 Both bounds on 0.3.0's code.
- [x] 3.2 Both tests failed on rust-0.3.0-rc4's code (`evidence/m18/before.log`).
- [ ] 3.3 Gate e2e-6 sends an oversized relay body on rust-0.3.0-rc5: 413, with RSS flat (`e2e6.pressure`).
