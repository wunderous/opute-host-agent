# m30: placeholders on user paths

Goal: `.agents/goals/m30-no-placeholders.md`. Measured on main `ea4b429`
(0.5.2). Every published MCP tool already had a handler (seam verifier: 144
compliant). These placeholders sat outside that check.

| # | Placeholder | Decision | Change | Evidence |
| --- | --- | --- | --- | --- |
| 1 | Provider teardown failed for every named unit ("host service operations are not implemented") | Fix, as Go's `cleanupProviderHostService` | `reclaim-provider-host-service` | Unit tests `provider::runtime::tests::teardown_*` (3 of 4 failed first); journey `e2e2.provider-teardown` fails on 0.5.0 (`m30-before`) and passes on 0.5.3-rc1 (`m30-r2`, gate `e2e-teardown`) |
| 2 | `mcp-exposure.v1` activation always failed | Fix: run the existing `mcpprobe` proof, as Go did | (in the same release) | `mcp_exposure_activation_*` (both failed first) |
| 3 | Tailscale `remove-ha-endpoint` reported `targetUri: vm:local:placeholder` for an unknown reference (Go did too) | Fix: no invented target | `truthful-ha-endpoint-removal` | `removing_an_unknown_ha_endpoint_names_no_target` (failed first) |
| 4 | CLI `public-mcp`, `recipe` and `provider` always fail, though the docs list them | **Owner decision**: wire (A, recommended) or withdraw (B) | `cli-operation-commands` (proposed, not implemented) | Go wrapped `ensure_public_mcp_tunnel`, the recipe tools and `opute.provider.*` |
| 5 | `tools.rs` doc said some handlers were unported | Fixed | none | none |
| 6 | Internal descriptors `configure_host_network` and `install_sql_forward_sidecar`: defined, unpublished, unported | Kept. They are Go's captured internal descriptors; a catalog test names one; a plan that names them is refused with the typed `not_implemented` error | none | seam report: `unported` and unpublished |

## Release candidate

- `rust-0.5.3-rc1`, built from `7f66afd2d` (every later commit on the branch
  is harness or evidence only). SHA256SUMS sha256 `173f9983…1df1e`.
- Checks before the build: `make rust-check`, `make e2e-test`, `make fuzz
  FUZZ_SECONDS=20` (all 18 targets), and `make spec-validate`.
- Dev VM built on it (`E2E_VERSION=0.5.3`). Runs:
  - `m30-r2/e2e2.provider-teardown`: pass, gate `e2e-teardown`;
  - `m30-r1/e2e2.host-ops`: pass. It exercises `set_host_service_state`,
    whose manager reload was split out for teardown.
- Not rerun on the candidate: the other 63 journeys. The changes touch
  provider teardown (no other journey calls it), the `mcp-exposure.v1`
  activation flow (no journey activates it), and the Tailscale fake backend
  (unit tests). The gates on `m28-r1` still verify.

## Findings along the way

- **Baseline drift.** On the 0.5.0 VM, a checked restore now loads the
  `tls` kernel module when Incus brings up its networks at boot. That never
  happened in the m28 runs, and the guest clock is three days past the
  snapshot. `m30-before` carries that harness error. The candidate VM's new
  baseline restores equal.
- **The VM build hard-coded its version** (`devvm.py VERSION = "0.5.0"`). It
  now reads `E2E_VERSION`.
- **Teardown is a plan run, not a task.** The journey polls
  `get_host_plan_run`. Afterwards `opute.provider.status` reports
  `active: false, connected: false`.
