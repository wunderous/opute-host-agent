## ADDED Requirements

### Requirement: Four tools without a Host Agent owner are not published
The Rust Host Agent SHALL NOT publish `register_kubernetes_cluster`, `list_agents`, `install_provider_tools` or `uninstall_provider_tools` in any profile, and a call naming one SHALL be answered as for any unknown tool (owner decision D37).

#### Scenario: Not listed and not callable
- **WHEN** a client lists tools and then calls `list_agents`
- **THEN** none of the four names is in `tools/list`, and the call gets the unknown-tool error
