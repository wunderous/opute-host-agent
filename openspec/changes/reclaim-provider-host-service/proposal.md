# Provider teardown reclaims the provider's host service

Status: **proposed (m30)**. A defect fix: it restores Go v0.2.4's
`cleanupProviderHostService` and adds no capability, tool, input or default.

## Why

`opute.provider.teardown` asks the provider for a teardown plan, runs it, has
the provider finalize, and then reclaims the provider's own systemd unit before
it retires the generation. In every Rust release up to 0.5.2 that last host
step was a placeholder: for any named unit it failed with

    inspect provider service "<name>": host service operations are not implemented by this Host Agent build

The agent fills in `serviceName` itself when it finds the provider's unit
(`providerTeardownInputs`). A teardown without a unit has to pass
`hostService: "none"`, otherwise it is refused. So a provider that runs as a
systemd unit, which is how providers are installed, could never be torn down.
The generation stayed active and the unit kept running. No journey called
`opute.provider.teardown`, so nothing caught it. The host-service operations
the step needs (inspect, state change, file removal, manager reload) have
shipped since M8a.

No requirement covered teardown, so the defect broke no written contract. This
change writes the contract down.

## What Changes

- After the provider's finalize succeeds, the agent reclaims the named unit in
  Go's order, so a failed step can be retried:
  1. inspect it;
  2. disable it if it is enabled;
  3. remove its unit file (`serviceFile`, or by default
     `~/.config/systemd/user/<name>` for scope `user` and
     `/etc/systemd/system/<name>` for scope `system`);
  4. reload the scope's manager;
  5. stop it if it was active.
- An inspection that reports no `active` or `enabled` state fails the
  teardown, naming the unit, and nothing is changed.
- Without a named unit, `hostService: "none"` is still the only way to
  skip the host step.

## Impact

- `crates/host-agent/src/provider/runtime.rs` (`cleanup_host_service`) and
  `crates/host-agent/src/hostservice.rs` (`reload_service_manager`, split out
  of `set_host_service_state`).
- No catalog, schema or wire change: `tools/list` is unchanged.
- Journey `e2e2.provider-teardown`: a k3s provider run as a user unit is
  installed, activated and torn down through typed calls.
