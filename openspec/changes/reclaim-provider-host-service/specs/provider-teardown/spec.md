## ADDED Requirements

### Requirement: Provider teardown reclaims the provider's host service

After a provider's teardown plan has succeeded and the provider has finalized,
the Host Agent SHALL reclaim the systemd unit named by the teardown's
`serviceName` before it retires the provider generation. It SHALL inspect the
unit; disable it if it is enabled; remove its unit file (`serviceFile`, or by
default `~/.config/systemd/user/<serviceName>` for scope `user` and
`/etc/systemd/system/<serviceName>` for scope `system`); reload the scope's
service manager; and stop the unit if it was active, in that order. If any step
fails, the teardown SHALL fail with an error naming the step and the unit, and
the generation SHALL stay active. With no `serviceName`, the host step SHALL be
skipped only when the caller passed `hostService: "none"`.

#### Scenario: A provider running as a user unit is torn down
- **WHEN** a provider whose unit is enabled and active in the user scope is torn down with `confirm: true` and its `serviceName`
- **THEN** the teardown completes, the unit file is gone, the unit is inactive, the provider's port is closed, and the provider's capabilities leave `tools/list`

#### Scenario: An incomplete inspection
- **WHEN** inspecting the named unit reports no `active` or `enabled` state
- **THEN** the teardown fails naming the unit, and the unit is neither disabled, removed nor stopped

#### Scenario: No unit to reclaim
- **WHEN** the teardown names no unit and passes `hostService: "none"`
- **THEN** no host service is inspected or changed
