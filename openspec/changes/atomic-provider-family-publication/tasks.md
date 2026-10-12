## 1. Review and reproduction

- [x] 1.1 Capture the pinned Go failure through its owning publication function and actual SDK tool listing.
- [x] 1.2 Obtain the owner's decision on the failure-preservation contract and baseline adoption. Decided 2026-10-12 as D16: atomic in Rust, Go unchanged.

## 2. Implementation and validation after approval

- [x] 2.1 Implement candidate validation and family displacement as one commit boundary. Registry `replace_*_displacing`; mounts displace after the candidate commits (`mount_displacing`).
- [x] 2.2 Preserve all publication projections and executable bindings on every pre-commit failure. Unit test `rejected_candidate_keeps_family_owner_and_valid_one_displaces_it` (fails against displace-first).
- [x] 2.3 Validate multi-provider activation rollback, accepted calls, and later unrelated publication through public MCP. Scenarios `provider-lifecycle.family.rejected-candidate-keeps-owner`, `.rollback-restores-owner` and `.replacement-keeps-accepted-call`; a Rust build without `restore_owners` fails the rollback scenario's Rust expectations.
- [ ] 2.4 Record the approved contract difference and provenance-bound Go/Rust evidence without waiving other parity gates.
