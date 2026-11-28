## ADDED Requirements

### Requirement: A refused Incus socket names the remedy

When an Incus command the Host Agent runs fails because the client may not
use the daemon's socket, the agent SHALL keep the Incus client's text and
append what to do. If the agent's user is in `incus-admin` but the agent's
process isn't, the agent SHALL say that it started before the membership and
must be restarted from a new login. If the user isn't in `incus-admin`, it
SHALL name `install_virtualization_stack` and `sudo usermod -aG incus-admin
<user>`, followed by a new login and a restart. Other Incus errors are
unchanged.

#### Scenario: Incus installed while the agent runs
- **WHEN** `install_virtualization_stack` has added the agent's user to `incus-admin` and an Incus command is refused the socket
- **THEN** the error says the user is in incus-admin, the agent started before that, and to log in again and restart the agent

#### Scenario: A user without access
- **WHEN** the agent's user isn't in `incus-admin` and an Incus command is refused the socket
- **THEN** the error names install_virtualization_stack and `sudo usermod -aG incus-admin <user>`, then a new login and a restart
