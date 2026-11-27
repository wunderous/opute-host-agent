## 1. Tests first

- [x] 1.1 Unit tests in `provider::runtime::tests` (`teardown_*`) with recording host-service operations: Go's order for an active, enabled user unit; no disable or stop for an idle system unit; an incomplete inspection refused; `hostService: "none"`. Three of the four fail against 0.5.2's placeholder.

## 2. Fix

- [x] 2.1 `cleanup_host_service` performs inspect, disable, remove, reload and stop through the shipped host-service operations.

## 3. Evidence

- [ ] 3.1 Journey `e2e2.provider-teardown`, run once on the published 0.5.0 binary, which fails at teardown (the reproduction).
- [ ] 3.2 The same journey passes once on the release candidate.

## 4. Release

- [ ] 4.1 With the owner's sign-off, the candidate is published as 0.5.3.
