# remove_vm_network_device

Status: **accepted (owner, 2026-11-11) as D36.** Goal m15.

## Why

The catalog publishes `remove_vm_network_device` ("Remove a network interface
device from a VM Incus configuration"; inputs `uri`, `deviceName`; output
`uri`, `vmName`, `deviceName`, `removed`; approval required; normal
admission cost). Go v0.2.4 has no handler and Rust answers
`not_implemented`. It is the undo of `configure_network` (D35).

## What Changes

A built-in Service Provider that removes one `nic` device from the bound
instance's own configuration. The declared schemas are unchanged.

## Impact

`incusops.rs` and the service context; the NIC journey on a dev-VM guest.
