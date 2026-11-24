## ADDED Requirements

### Requirement: configure_network attaches one bridged NIC
The Rust Host Agent SHALL answer `configure_network` for the instance the call's `uri` resolves to by adding an Incus device named `deviceName` (default `eth1`) of type `nic`, `nictype` `bridged`, with `parent` (default `wlan0`), crossing the agent's admission boundary with the descriptor's declared cost, and SHALL return `uri`, `vmName`, `deviceName`, `parent` and `status: "attached"` (owner decision D35). A call naming a device the instance already has with the same type and parent SHALL succeed without change. A device of that name with a different configuration, a parent the host does not have, or an instance that does not resolve SHALL end the call with a typed tool error and leave the instance unchanged.

#### Scenario: Attach and attach again
- **WHEN** a client calls `configure_network` twice for the same instance with `parent` set to an existing host bridge
- **THEN** both calls return `status: "attached"`, and the instance has exactly one such device

#### Scenario: Conflicting device
- **WHEN** the instance already has a device `eth1` of another type or parent
- **THEN** the call ends with a typed tool error and the device is unchanged
