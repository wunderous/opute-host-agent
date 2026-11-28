# A refused Incus socket names the remedy

Status: **proposed (m33)**. A defect fix found by journey `e2e9.readme`. It
adds no tool, input, output field or default.

## Why

README step 4 has a newcomer ask for *"Install Incus and create a small
Ubuntu VM called sandbox"*. For a sudo user who isn't in `incus-admin`,
`install_virtualization_stack` installs the packages and runs
`usermod -aG incus-admin <user>`. A running process keeps the groups it
started with, so the agent still can't use the daemon's socket. The same call
then fails while initializing the container runtime, and so does every later
Incus tool, with only the Incus client's text:

    You don't have the needed permissions to talk to the incus daemon
    (socket path: /var/lib/incus/unix.socket)

Nothing tells the user or the assistant that the fix is to log in again and
restart the agent (draft `m33-draft1`). The agent can't take the group itself:
the new membership applies only to a new login session.

## What Changes

- When an Incus command fails with that refusal, the agent appends what to do,
  from the group database and its own process groups:
  - the user is in `incus-admin` but this process isn't: *"<user> is in
    incus-admin, but this agent started before that: log in again and restart
    the agent"*;
  - the user isn't in `incus-admin`: *"<user> is not in incus-admin:
    install_virtualization_stack adds it, or run `sudo usermod -aG incus-admin
    <user>`; then log in again and restart the agent"*.
- A refusal for any other reason, and every other Incus error, is unchanged.
- README step 4 says to log in again and restart the agent after Incus is
  installed for the first time.

## Impact

- `crates/host-agent/src/incus.rs`: the Incus command runner.
- Error text only: `tools/list` is unchanged.
