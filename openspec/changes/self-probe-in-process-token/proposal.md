# The public-MCP connector probes its origin with an in-process token

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D52**. Goal m21, finding U1. Applies to 0.3.0 from `rust-0.3.0-rc7`. 0.4.0 is unchanged.

## Why

D8's [`oauth-issuance`](../secure-oauth-issuance/specs/oauth-issuance/spec.md)
contract says a self-probe mints its token in the agent's own store, binds it
to the probed resource for at most five minutes, revokes it afterwards, and
never calls the agent's own token endpoint. In 0.3.0, the public-MCP
connector's probes still POST `client_credentials` for `opute-mcp-host` to
`/oauth/token` with no secret, as Go v0.2.4 did. D8 refuses that grant
(`invalid_client`), so `ensure_public_mcp_tunnel` and
`ensure_public_mcp_quick_tunnel` start the connector and then always fail
with "probe public MCP origin: MCP OAuth token request returned HTTP 401".
This happens whether or not `OPUTE_HOST_OAUTH_CLIENT_SECRET` is set.

`e2e4.upgrade-chain` found the defect on rust-0.3.0-rc6 (`m21-draft4`):
- rc6 fails the call both before the upgrade and after rolling back from
  0.4.0;
- rust-0.4.0-rc4 gets past the origin on the same host, because
  platform-neutral (N4) probes as the configured machine client.

m19's `e2e6.secret-sweep` hit the same 401 on rc6 but accepted a failed
connector call, so it was not reported then.

## What Changes

In 0.3.0, the origin probe and the public-endpoint probe of both connector
tools use a token that the agent mints in its `authz.sqlite`:
- the token belongs to client `opute-mcp-host`;
- it is bound to the probed resource (the probe URL, normalized as before);
- it lives 300 s and is revoked when the probe ends, whatever the outcome.

The observation reports `authorizationMode: "in-process"`. Nothing is sent to
the token endpoint. The inputs, outputs and settings stay the same.

## Options rejected

- **Send `OPUTE_HOST_OAUTH_CLIENT_SECRET` to the token endpoint.** That is
  what N4 does in 0.4.0. In 0.3.0 it breaks D8's rule that a self-probe never
  calls the token endpoint, and it still fails when the secret is unset.
- **Report the origin as unverified, as 0.4.0 does without a machine
  client.** That is a new outcome for a 0.3.0 tool, and so a contract
  change.
- **Leave it.** A connector that 0.4.0 reconciles could not be reconciled
  again after a rollback to 0.3.0.

## Impact

- `hostpublic.rs` (the probes), `oauth.rs` (`mint_self_probe`), and
  `tools::Host.authz_dir`.
- The test `hostpublic::tests::the_origin_probe_uses_an_in_process_token`,
  which fails on rc6's code (`evidence/m21/before-d52.log`).
- When main is merged, `rust-0.4.0` keeps its own N4 probe.
