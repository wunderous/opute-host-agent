## ADDED Requirements

### Requirement: The launcher provides the workload slice for mutations

When the npm launcher starts the agent with `--allow-mutations`, in the
foreground or in the background, it SHALL ensure that the systemd user unit
`opute-workload.slice` is defined before the agent starts (owner decision
D56). If systemd already loads a definition of that slice for the user, the
launcher SHALL write nothing. Otherwise it SHALL write
`$XDG_CONFIG_HOME/systemd/user/opute-workload.slice`
(`~/.config/systemd/user/` when `XDG_CONFIG_HOME` is unset), with exactly the
text the agent renders for that slice under the current resource policy
revision, and reload the user manager. If the user manager cannot be
reached, the launcher SHALL exit with an error before starting the agent.
Without `--allow-mutations`, the launcher SHALL NOT read or write any
slice. The launcher SHALL NOT change the agent's resource enforcement
setting.

#### Scenario: First start with mutations
- **WHEN** a user with a systemd user session and no `opute-workload.slice` runs `start --allow-mutations`
- **THEN** the launcher writes the slice with the agent's rendered limits, reloads the user manager, names the file on stderr, and starts the agent; a mutation that is not of the `control` class is no longer refused with `host_resource_enforcement_unknown`

#### Scenario: Existing slice is kept
- **WHEN** systemd already loads an `opute-workload.slice` for the user
- **THEN** the launcher writes nothing and starts the agent

#### Scenario: No user session
- **WHEN** `systemctl --user` fails and the slice does not exist
- **THEN** the launcher exits 1 without starting the agent, and its message names the slice and its limits

#### Scenario: Read-only start
- **WHEN** the launcher starts without `--allow-mutations`
- **THEN** it runs no `systemctl` command and writes no file
