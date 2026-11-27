# An unknown HA endpoint's removal names no target

Status: **proposed (m30)**.

## Why

The Tailscale provider's `remove-ha-endpoint` is idempotent: removing a
reference it does not hold succeeds with `deleted: true`. That result also
carries `"targetUri": "vm:local:placeholder"`, which names a resource that does
not exist. Go v0.2.4 returns the same value, and the Rust port kept it. A
caller that records or acts on `targetUri` would act on a made-up VM. The
Cloudflare provider refuses an unknown reference instead.

## What Changes

- For a reference the provider does not hold, the result keeps `deleted:
  true` and `ready: true`, carries the caller's `endpointRef`, and has no
  `targetUri`.
- A known reference's result is unchanged; it also carries `endpointRef`, as
  it did before.
- The output schema already marks `targetUri` optional, so `tools/list` does
  not change.

## Impact

- `crates/host-agent/src/providers/tailscale/overlay.rs` and `live.rs`
  (`ha_endpoint_removed`).
- Unit test `providers::tailscale::tests::removing_an_unknown_ha_endpoint_names_no_target`.
