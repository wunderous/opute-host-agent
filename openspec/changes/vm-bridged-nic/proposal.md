# configure_network: attach a bridged NIC to an instance

Status: **accepted (owner, 2026-11-11) as D35.** Goal m15.

## Why

The catalog publishes `configure_network` ("Attach a bridged NIC device to a
VM so it joins a host network bridge"; inputs `uri`, `deviceName` default
`eth1`, `parent` default `wlan0`; output `uri`, `vmName`, `deviceName`,
`parent`, `status: attached`; approval required; normal admission cost).
Go v0.2.4 has no handler and Rust answers `not_implemented`.

## What Changes

A built-in Service Provider: an Incus `nic` device of type `bridged` with the
given parent added to the bound instance. The published defaults are kept as
published. The declared schemas, approval and cost are unchanged.

## Impact

`incusops.rs` and the service context; a NIC journey on a dev-VM guest.
