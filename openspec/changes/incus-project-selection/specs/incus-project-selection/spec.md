## ADDED Requirements

### Requirement: One Incus project per agent
The Host Agent SHALL address one Incus project in every Incus call it makes for
an operation, including raw API queries, so that an instance it creates is the
instance it then observes, starts, stops and deletes.

#### Scenario: The client's current project is not default
- **WHEN** the agent's Incus client has a current project other than `default` and a client calls `provision_vm` for a container
- **THEN** the container is created and observed in the same project, and the call does not fail because the address wait read another project
