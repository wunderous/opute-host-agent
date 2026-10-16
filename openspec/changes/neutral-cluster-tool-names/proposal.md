# Provider-neutral cluster tool names

## Why

D19 and D21 renamed the host and instance tools that named an
implementation. The M8c kubernetes and cluster domains have nine published
tool names that do the same (`k8s`, `kubernetes`, `helm`). Under D14 only a
Service Provider names the technology behind a capability, so these names
must become neutral before the domains are ported.

**Decided (owner, 2026-10-17), recorded as D22.** This change extends D19's
name map, with the same rules: neutral published names, and Go names kept as
hidden, deprecated aliases until cutover.

## What Changes

| Go name (alias) | Published Rust name |
| --- | --- |
| `get_k8s_resource` | `get_cluster_resource` |
| `delete_k8s_resource` | `delete_cluster_resource` |
| `get_k8s_resource_status` | `get_cluster_resource_status` |
| `list_k8s_events` | `list_cluster_events` |
| `put_k8s_secret` | `put_cluster_secret` |
| `exec_kubernetes_command` | `exec_cluster_command` |
| `list_kubernetes_clusters` | `list_compute_clusters` |
| `install_helm_chart` | `install_cluster_package` |
| `render_helm_template` | `render_cluster_package` |

`list_compute_clusters` avoids `list_clusters`, which is already a Go tool
name. That tool, `get_cluster_details`, `get_cluster_runtime_details` and
`uninstall_helm_chart` are registered in Go but never published. They keep
their names and Rust refuses them exactly as Go does.

No input field names a provider, so no field is renamed. No output key,
effect, admission class, default or behaviour changes. A description that
names a renamed tool uses its new name; otherwise descriptions keep Go's
text, including "Kubernetes" and "Helm" where they name the product.

An alias works exactly as under D19:

- It is absent from `tools/list` and `get_capability_catalog`, but
  `tools/call` accepts it.
- It takes Go's input schema.
- It reaches the same Consumer and returns the same result.
- Its operation and audit rows record the name that was called.

## Capabilities

### New Capabilities

- `neutral-cluster-tool-names`: the kubernetes- and cluster-domain entries
  in the one name map, and their evidence.

### Modified Capabilities

None. Go is unchanged.

## Impact

- **Name map:** the entries join `crates/host-agent/catalog/neutral-names.json`.
- **Catalog revisions:** every Rust catalog revision changes again. Each is
  re-pinned in `parity-manifest.json` with its Go pair, as D19 requires.
- **Divergence:** `D19.neutral-host-tool-names` reads the map, so it covers
  the new entries without a second rule. Any other difference stays a diff.
- **Compatibility:** Platform and recipes keep working through the aliases
  until cutover.
