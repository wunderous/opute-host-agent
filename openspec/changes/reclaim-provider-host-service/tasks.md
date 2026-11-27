## 1. Tests first

- [x] 1.1 Unit tests in `provider::runtime::tests` (`teardown_*`) with recording host-service operations: Go's order for an active, enabled user unit; no disable or stop for an idle system unit; an incomplete inspection refused; `hostService: "none"`. Three of the four fail against 0.5.2's placeholder.

## 2. Fix

- [x] 2.1 `cleanup_host_service` performs inspect, disable, remove, reload and stop through the shipped host-service operations.

## 3. Evidence

- [x] 3.1 Journey `e2e2.provider-teardown`, run once on the published 0.5.0 binary, which fails at teardown (the reproduction): `evidence/e2e/runs/m30-before`.
- [x] 3.2 The same journey passes once on the release candidate: `m30-r2` on `rust-0.5.3-rc1`, gate `e2e-teardown`.

## 4. Release

- [x] 4.1 With the owner's sign-off (2026-11-26), the candidate is published as 0.5.3: opute-host-agent v0.5.3 on `7f66afd2d`, host-agents#42 (pin, Publish run 45932146871, npm latest 0.5.3), MCP Registry run 45932443611, site promotion host-agents#43.
