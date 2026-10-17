## ADDED Requirements

### Requirement: Cluster tools follow the neutral name map
The Rust Host Agent SHALL publish `get_k8s_resource`, `delete_k8s_resource`, `get_k8s_resource_status`, `list_k8s_events`, `put_k8s_secret`, `exec_kubernetes_command`, `list_kubernetes_clusters`, `install_helm_chart` and `render_helm_template` as `get_cluster_resource`, `delete_cluster_resource`, `get_cluster_resource_status`, `list_cluster_events`, `put_cluster_secret`, `exec_cluster_command`, `list_compute_clusters`, `install_cluster_package` and `render_cluster_package` (owner decision D22, extending D19). No other name, field, output key, effect, admission class, default or behaviour SHALL change.

#### Scenario: The catalog lists the neutral names
- **WHEN** a client lists tools in any serving mode
- **THEN** each neutral name appears where Go publishes its Go name, and no Go name from this map appears

#### Scenario: Unpublished Go tools stay unreachable
- **WHEN** a client calls `list_clusters`, `get_cluster_details`, `get_cluster_runtime_details` or `uninstall_helm_chart`
- **THEN** Rust refuses the call exactly as Go does, and none of them is listed

### Requirement: Go names remain accepted until cutover
`tools/call` SHALL accept each Go name in this map as a hidden alias with Go's input schema, reaching the same Consumer and recording the called name. No alias SHALL appear in a published catalog.

#### Scenario: A Platform call with a Go name
- **WHEN** a client calls `install_helm_chart` with Go's arguments
- **THEN** it behaves exactly as the same call to `install_cluster_package`

### Requirement: The difference stays declared and exact
`D19.neutral-host-tool-names` SHALL map these names and each pinned Rust revision back to Go's spelling on the Rust side only. Any other difference SHALL remain a diff.

#### Scenario: Renamed catalog compared with Go
- **WHEN** a catalog scenario runs Go against Rust
- **THEN** the divergence applies and is not stale, and the observations are identical
