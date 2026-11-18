# The public-MCP connector reads its tunnel token from its environment

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D47**, under D46 ([`secret-placement`](../secret-placement/proposal.md)). Goal m19, finding S1. Applies to 0.3.0 from `rust-0.3.0-rc6` and to 0.4.0 from `rust-0.4.0-rc4`.

## Why

`ensure_public_mcp_tunnel` stores the tunnel token in a 0600 env file and
installs a unit whose `ExecStart` is `/bin/sh -c 'exec cloudflared tunnel
--no-autoupdate run --token "${OPUTE_CLOUDFLARED_TUNNEL_TOKEN}"'`. The shell
expands the variable, so the token is on cloudflared's argv, which every
local user reads from `/proc`. cloudflared restarts every 15 s while the
tunnel is down, so it is there again each time. `e2e6.secret-sweep` saw it
on rust-0.3.0-rc5 and rust-0.4.0-rc3 (finding S1).

## What Changes

The unit's command becomes `TUNNEL_TOKEN="${OPUTE_CLOUDFLARED_TUNNEL_TOKEN}";
export TUNNEL_TOKEN; exec cloudflared tunnel --no-autoupdate run`.
cloudflared reads its token from `TUNNEL_TOKEN` when `--token` is absent.
The token file keeps its path, mode and its one key, so the CLI's
`--token-file` and existing files keep working; an existing unit is
rewritten on the next ensure. The tool's result does not change.

## Options rejected

- **`--token-file`.** The token file holds `KEY=value` for the unit's
  `EnvironmentFile=`; a second, bare token file would hold the same secret
  twice.
- **Renaming the file's key to `TUNNEL_TOKEN`.** The file is a published
  format the CLI reads; changing it buys nothing the export does not.

## Impact

`hostpublic.rs` (`tunnel_exec`), and the test
`hostpublic::tests::the_tunnel_token_never_reaches_cloudflareds_arguments`,
which runs the unit's command with a stub cloudflared; it failed on rc5's
and rc3's code.
