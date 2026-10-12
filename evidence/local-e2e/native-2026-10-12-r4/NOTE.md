# r4: m5, m6 and cutover failed on M5 shape (kept, not relabeled)

The cycle ran on the committed tree `12396739` with a clean worktree before
and after. Go/Go was 177/177 over 20 iterations and Go/Rust 177/177 over
five. Canaries, contracts, oracles, the go-sdk OAuth client and gates m0-m4
passed. M5 shape was 860/870, so m5, m6 and cutover failed. Two scenarios
each failed all five runs:

- `config.env.resource-policy` found a Rust defect. Go records the
  configured `OPUTE_HOST_RESOURCE_POLICY_REVISION` in every invocation's
  durable execution binding (`hostmcp/server.go:580,614`). Rust recorded
  the default `opute-host-resource-policy.v2`. The capacity report was
  already correct, which is why the wire comparison passed. Fixed: the
  bindings now take the coordinator's resolved revision.
- `admission.restart.plan-run-reservation-after-kill` persisted node n1's
  `get_host_info` output in the plan run's state. That output carries Go's
  map-ordered `supportedTools` (finding F-2) and live host telemetry. The
  F-2 set rule now also covers plan-run node outputs, with a test showing a
  membership difference still fails. The node is now a deterministic
  `inspect_host_file`, since it only needs to complete before n2 waits.

The rerun is `native-2026-10-12-r5`.
