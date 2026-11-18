# Every Cloudflare connector reads its run token from a 0600 file

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D48**, under D46 ([`secret-placement`](../secret-placement/proposal.md)). Goal m19, finding S2 and S6. Applies to 0.3.0 from `rust-0.3.0-rc6` and to 0.4.0 from `rust-0.4.0-rc4`.

## Why

Two connectors the Cloudflare provider installs carried the run token on
cloudflared's argv. The host-placed connector (`reconcile_tunnel` with
`placement: host` and no `endpoint`) wrote `ExecStart=… run --token
<token>` into its unit (S2). The HA connector installed in each guest by
`ensure_ha_endpoint` wrote the same line into a unit created with the
shell's umask, 0644, so the token was also in a world-readable file (S6).
The container and Kubernetes placements already use `--token-file`.
Unit tests found both: the dev VM has no Cloudflare account to drive them.

## What Changes

Both write the run token to a 0600 file and start cloudflared with
`--token-file`:

- host placement: `/etc/opute-cloudflare/<service>.token` (system scope) or
  `~/.config/opute-cloudflare/<service>.token` (user scope), written by
  `ensure_host_file` (its `content` is `writeOnly`, so the store keeps no
  copy) and removed with the unit by `remove_tunnel`;
- HA connector: `/etc/opute-cloudflare/<service>.token` in the guest,
  written under `umask 077` from stdin, and removed with the unit.

`<service>` is the unit name without `.service`. The pinned cloudflared
supports `--token-file` (2025.4.0 and later).

## Options rejected

- **`Environment=TUNNEL_TOKEN=` in the unit.** Any local user reads a
  unit's `Environment=` over D-Bus (`systemctl show`).
- **Keeping the token in the unit and making the unit 0600.** That still
  leaves it on argv.

## Impact

`providers/cloudflare/mod.rs` (`reconcile_legacy_host_tunnel_with`,
`host_token_file`, `remove_tunnel`) and `mesh.rs` (`HA_CONNECTOR_SCRIPT`,
`ha_connector_script`, `remove_ha_connector`); the tests
`a_host_connector_never_carries_its_run_token_on_argv` and
`an_ha_connector_keeps_its_run_token_in_a_private_file` failed on rc5's
code (`evidence/m19/before.log`, `before-s6.log`).
