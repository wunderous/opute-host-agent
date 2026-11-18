## ADDED Requirements

### Requirement: The cluster agent's config never travels on argv
`install_cluster_agent` SHALL send the cluster agent's config on the install command's standard input, never in its arguments, raw or encoded, and SHALL write it with mode 0600 (owner decision D50).

#### Scenario: A host install
- **WHEN** a cluster agent is installed on the host through `sudo`
- **THEN** no process's arguments and no journal entry hold the bridge token, and `/etc/opute/cluster-agent.json` has mode 0600
