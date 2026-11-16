## 1. Decision

- [x] 1.1 Decide the validation-only node found in goal m20 (delegated by the owner, 2026-11-11): it validates, D45.
- [x] 1.2 Add D45 to the migration's owner decision table.

## 2. Implementation and evidence

- [x] 2.1 `run_node` and `run_for_each_item` evaluate the validation of a node without an action.
- [x] 2.2 Failing-first test (failed on rc2's code, `evidence/m20/d45-before.log`).
- [ ] 2.3 Gate e2e-7 runs the site recipe, whose `guest-address`, `connector` and `public` nodes validate after applied ones, on rust-0.4.0-rc3: `e2e7.public-site`.
