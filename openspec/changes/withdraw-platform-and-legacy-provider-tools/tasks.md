## 1. Decision

- [x] 1.1 Owner decision: withdraw the four tools. Decided 2026-11-11 as D37.
- [x] 1.2 Add D37 to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 Remove the four from the served catalog and pin its revision. Done as `catalog::WITHDRAWN`, filtered out of `served()`; `catalog/source.json` keeps Go v0.2.4's record, and the seam compares Go's captured `tools/list` without them.
- [x] 2.2 Remove their unported seam registrations; the seam verifier reports no gap. They had no service, so nothing was registered; the report lists them unpublished.
- [x] 2.3 A test (`seam::withdrawn_tools_are_unknown`): none of the four is listed, and a call gets the unknown-tool error.

## 3. Evidence

- [x] 3.1 The 0.4.0 candidate's `tools/list` in every gate omits them. Passed: every 0.4.0 `tools/list` in run `m15-r1` on `rust-0.4.0-rc1` omits the four; only `neutral.rollback`'s 0.3.0 lists name them.
