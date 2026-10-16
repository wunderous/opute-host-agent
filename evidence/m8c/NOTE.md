# M8c entry: blocked on M8b and one owner decision

Checked 2026-10-17 on `claude/m8b-incus-domain` at `a80c433c`.

## 1. M8b is not done yet

M8b round 1 was interrupted by a host reboot during `t2-go-vs-go` (log ends
2026-10-14 02:48 with no exit line). Every step that finished exited 0, and
T2 Go/Go had logged no failure. It is retired under
`.parity/retired/m8b-round1/` and is not counted. Round 2 was started on the
same head on 2026-10-17. M8c starts only once round 2 is clean,
`evidence/m8b/verify.py` reproduces it, and M8b is merged.

## 2. The M8c rename map (owner decision)

These are the Go kubernetes and cluster tools from
`internal/tools/dispatch_kubernetes.go` and `dispatch_cluster.go`. The
catalogs come from `baseline/inventory/catalog/*.json`, and reachability
from `baseline/inventory/dispatch-registry.json`.

No input field in these tools names a provider (`chart`, `repo`,
`manifest`, `kind` and `ingressClass` are generic terms), so this proposal
renames no fields.

| Go tool | Published? | Proposal |
| --- | --- | --- |
| `get_k8s_resource` | yes | **rename** to `get_cluster_resource` |
| `delete_k8s_resource` | yes | **rename** to `delete_cluster_resource` |
| `get_k8s_resource_status` | yes | **rename** to `get_cluster_resource_status` |
| `list_k8s_events` | yes | **rename** to `list_cluster_events` |
| `put_k8s_secret` | yes | **rename** to `put_cluster_secret` |
| `exec_kubernetes_command` | yes | **rename** to `exec_cluster_command` |
| `list_kubernetes_clusters` | yes | **rename** to `list_compute_clusters`. The obvious `list_clusters` is already a Go tool name (unreachable, see below), so reusing it would collide with an alias |
| `install_helm_chart` | yes | **rename** to `install_cluster_package` |
| `render_helm_template` | yes | **rename** to `render_cluster_package` |
| `uninstall_helm_chart` | no, unreachable | keep. It is not published, so there is no name to rename. Rust refuses it as Go does |
| `list_clusters`, `get_cluster_details`, `get_cluster_runtime_details` | no, unreachable | keep, and refuse as Go does |
| `apply_manifest`, `inspect_guest_storage`, `trim_guest_storage` | yes | keep |
| `configure_service_domain`, `remove_service_domain` | yes (not `platform`) | keep |
| `install_cluster_agent`, `prune_unused_cluster_images`, `garbage_collect_cluster_registry` | yes | keep |
| `configure_agent_connection` | yes | keep. M8a already ported it (`host-domain.connection.*`); M8c does not port it again |

As with D19 and D21:

- Tool descriptions keep Go's text, including "Kubernetes" and "Helm" where
  they name the product. Only renamed tool names inside descriptions are
  rewritten.
- The Go names stay hidden aliases with Go's input schema until cutover.
- The renames go into one OpenSpec change extending
  `crates/host-agent/catalog/neutral-names.json`, and the catalog revisions
  are re-pinned with their Go pairs.

Please approve, amend or reject each **rename** row.

## Resolved 2026-10-17

The owner approved the rename map as proposed. This is recorded as D22
(`openspec/changes/neutral-cluster-tool-names`). Entry item 2 is met; entry
item 1 waits on M8b round 2.

## Addendum 2026-10-17: seven more kubernetes-domain tools

The inventory (`tools/parity/cluster_domain.py`) finds seven published
tools in `dispatch_unassigned.go` whose handlers call `svc.Kubernetes()`:
`list_namespaces`, `list_storage_classes`, `list_ingress_classes`,
`list_certificate_issuers`, `list_pods`, `list_services` and
`list_deployments`. They are in M8c's scope as part of the kubernetes
domain. None names a provider, so all keep their names; D22 does not change.
