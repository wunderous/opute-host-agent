## 1. Decision

- [x] 1.1 Owner decision: implement. Decided 2026-11-11 as D36.
- [x] 1.2 Add D36 to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 A built-in Service Provider on the D14 seam; the seam verifier reports the tool compliant.
- [x] 2.2 Unit tests for the outcomes below, failing first.
- [x] 2.3 Its entry point in `evidence/hardening/entry-points.json` with a fuzz target or property.

## 3. Evidence

- [x] 3.1 The NIC journey on a dev-VM guest, run on the 0.4.0 candidate. Passed: `e2e2.incus`, run `m15-r1` on `rust-0.4.0-rc1`.
