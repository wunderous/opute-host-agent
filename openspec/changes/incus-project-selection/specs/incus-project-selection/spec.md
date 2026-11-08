## ADDED Requirements

### Requirement: One Incus project per agent
The Host Agent SHALL address the `default` Incus project in every Incus call
it makes, including raw API queries and the K3s provider's calls, whatever
the client's current project is, so that an instance it creates is the
instance it then observes, starts, stops and deletes.

#### Scenario: The client's current project is not default
- **WHEN** the agent's Incus client has a current project other than `default` and a client calls `provision_vm` for a container
- **THEN** the container is created and observed in `default`, and the call does not fail because the address wait read another project
