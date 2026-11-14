## ADDED Requirements

### Requirement: remove_vm_network_device removes one NIC from an instance's own configuration
The Rust Host Agent SHALL answer `remove_vm_network_device` for the instance the call's `uri` resolves to, crossing the admission boundary with the descriptor's declared cost: a device `deviceName` of type `nic` in the instance's own configuration SHALL be removed and the call SHALL return `removed: true`; when the instance has no device of that name the call SHALL return `removed: false` without change (owner decision D36). A device of that name that is not a `nic`, a device the instance inherits from a profile, or an instance that does not resolve SHALL end the call with a typed tool error and leave the instance unchanged.

#### Scenario: Remove and remove again
- **WHEN** a client removes a NIC `configure_network` attached, then removes it again
- **THEN** the first call returns `removed: true`, the second `removed: false`, and the instance no longer has the device

#### Scenario: A profile device is not removed
- **WHEN** `deviceName` names the instance's profile `eth0`
- **THEN** the call ends with a typed tool error and the instance keeps its network
