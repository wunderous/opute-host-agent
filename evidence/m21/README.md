# m21: the release path (0.3.0 side)

Goal m21 takes one host along the path users will follow: Go v0.2.4, then
rust-0.3.0, then rust-0.4.0, then a rollback to rust-0.3.0. The journey is
`e2e4.upgrade-chain` in `rust-0.4.0`'s gate e2e-4, because only that line
holds both binaries' expectations. This directory holds the 0.3.0 side.

## Finding

| Id | Found | What | Decision |
|---|---|---|---|
| U1 | `e2e4.upgrade-chain` drafts on rust-0.3.0-rc6 and rust-0.4.0-rc4 (not committed) | `ensure_public_mcp_tunnel` and `ensure_public_mcp_quick_tunnel` requested a secretless `opute-mcp-host` token from the agent's own `/oauth/token`. D8 refuses that grant, so each call ended in "probe public MCP origin: MCP OAuth token request returned HTTP 401". This happened whether or not `OPUTE_HOST_OAUTH_CLIENT_SECRET` was set, both before the upgrade and after rolling back from 0.4.0. 0.4.0 got past the origin on the same host. | D52, [`self-probe-in-process-token`](../../openspec/changes/self-probe-in-process-token/proposal.md) |

m19's `e2e6.secret-sweep` met the same 401 on rc6 but accepted a failed
connector call. On rc7 (`m21-r1`), that call gets past the origin and stops
only at its `.invalid` public endpoint.

## Evidence

- `before-d52.log`: `hostpublic::tests::the_origin_probe_uses_an_in_process_token`,
  adapted to rc6's signature, fails on rc6's code with the journey's error.
- `fuzz.txt`: a clean 60 s pass over all 17 targets on rc7's product commit.
- Candidate `rust-0.3.0-rc7` (cf151caa7): every gate passes on run `m21-r1`.
  The contract suites, seam stage `m21`, the arm64 install and the
  distribution checks were recaptured on rc7.
