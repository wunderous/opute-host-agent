## 1. Decision

- [x] 1.1 Owner decision: withdraw the four tools. Decided 2026-11-11 as D37.
- [x] 1.2 Add D37 to the migration's owner decision table.

## 2. Implementation

- [ ] 2.1 Remove the four from the catalog source; regenerate the derived catalog and pin its revision.
- [ ] 2.2 Remove their unported seam registrations; the seam verifier reports no gap.
- [ ] 2.3 A test: none of the four is listed, and a call gets the unknown-tool error.

## 3. Evidence

- [ ] 3.1 The 0.4.0 candidate's `tools/list` in every gate omits them.
