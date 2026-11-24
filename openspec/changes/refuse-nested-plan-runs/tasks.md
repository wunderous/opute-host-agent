## 1. Decision

- [x] 1.1 Decide the nested-run deadlock found in goal m20 (delegated by the owner, 2026-11-11): refuse every tool that starts a plan run, D44.
- [x] 1.2 Add D44 to the migration's owner decision table.

## 2. Implementation and evidence

- [x] 2.1 `plan::schema::PLAN_RUN_TOOLS` lists the seven tools; plan and recipe validation refuse them.
- [x] 2.2 Failing-first tests for both validators (failed on rc2's code, `evidence/m20/d44-before.log`).
- [ ] 2.3 Gate e2e-7 runs a site recipe whose providers were installed by typed calls first, on rust-0.4.0-rc3: `e2e7.public-site`.
