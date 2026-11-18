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

## The 0.4.0 side

No 0.4.0 code changed, and `rust-0.4.0-rc4` stays the candidate.
`e2e4.upgrade-chain` joined gate e2e-4. The rollback journeys and the
candidate's `rollbackTarget` now name `rust-0.3.0-rc7`. The sweep's scan
accepts the agent's own user as the owner of a private file: the production
agent runs as `opute`.

Gates e2e-4, e2e-neutral and e2e-6 reran on rc4 under `m21-r2`, and every
journey passed on its first attempt. In the chain:
- Go 0.2.4's run stopped in flight reads back `cancelled` on 0.3.0 and on
  0.4.0;
- each connector call on rc7 and on rc4 stops only at the `.invalid`
  public endpoint;
- the three scans find no token in the journal, the store, a non-private
  file or another process's environment.

Four draft runs (`.parity/e2e/drafts-m21/`, not committed) found these
journey defects before U1:
- a task needs the user's systemd manager;
- the scan accepted only root as owner;
- a system-scoped connector and a host cluster agent need root, which the
  production agent does not have;
- the connector calls were asserted only to finish.

## minimal.opute.io through a rollback and back

After the gates, on 2026-11-18, the dev VM (built from `rust-0.4.0-rc4`) was
restored under the lease held as `minimal-opute-io`. The owner's credentials
file was copied in unread (0600). The m20 prerequisites ran, then the m20
site recipe: run `36f0e3f2-cfaa-43f4-b00b-7ee87b1b9a33` completed. The
tunnel still existed in Cloudflare, so its connector and public nodes were
already satisfied.

With a probe of `https://minimal.opute.io/` every 5 s
([`site-roundtrip-probe.log`](site-roundtrip-probe.log)), the agent on the
production unit was:
1. rolled back to `rust-0.3.0-rc7` (20:29:09 to 20:29:10 UTC, `0.3.0`
   active);
2. left there for 60 s;
3. moved forward to rc4 again (20:30:11 UTC, `0.4.0` active).

All 22 probes answered 200 over verified TLS.

Afterwards the site recipe ran again (`a449cc55-540f-4498-a676-9927edfb6830`,
`runNonce: after-roundtrip-m21`), and every node was satisfied. The site
stays up on the dev VM.
