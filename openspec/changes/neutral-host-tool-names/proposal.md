# Provider-neutral host tool names

## Why

Under D14 every capability has a Service Definition, a Service Provider and a
Consumer. Only a Service Provider should name the technology that implements
it. The pinned Go agent publishes eight host-domain tools whose names, and one
input field, name an implementation (Incus, WSL, Docker or k3d). Platform and
recipes therefore bind to an implementation, not to a capability.

**Decided (owner, 2026-10-12), recorded as D19.** Rust publishes
provider-neutral names and keeps each Go name as a hidden, deprecated alias.
The rename covers tool names, provider-named input fields, and
`ensure_docker` and `ensure_k3d`. The aliases are removed at cutover, in a
separate change.

## What Changes

| Go name (alias) | Published Rust name |
| --- | --- |
| `install_incus_stack` | `install_virtualization_stack` |
| `uninstall_incus_stack` | `uninstall_virtualization_stack` |
| `probe_incus_gpu` | `probe_virtualization_gpu` |
| `terminate_wsl_distribution` | `terminate_host_vm_distribution` |
| `shutdown_wsl` | `shutdown_host_vm_subsystem` |
| `compact_wsl_disk` | `compact_host_vm_disk` |
| `ensure_docker` | `ensure_container_engine` |
| `ensure_k3d` | `ensure_local_cluster_runtime` |

The input field `install_incus_stack.incusPackage` becomes
`install_virtualization_stack.packageName`. No other field, output key,
effect, admission class, default or behaviour changes. A description that
names a renamed tool uses its new name; descriptions are otherwise Go's text.

An alias is absent from `tools/list` and from `get_capability_catalog`, but
`tools/call` still accepts it. It takes Go's input schema, including
`incusPackage`, reaches the same Consumer, and returns the same result. Its
operation and audit rows record the name that was called, as Go does.

`ensure_k3d` is omitted from every catalog Go publishes on a Linux Incus host.
Its neutral name therefore changes only the internal and Windows catalog
projections.

## Capabilities

### New Capabilities

- `neutral-host-tool-names`: the host-domain name map, its aliases, and the
  evidence that pins it.

### Modified Capabilities

None. Go is unchanged.

## Impact

- **Catalog revisions:** the published tools/list differs from Go only by
  the map above, but every catalog revision Rust publishes changes. Each new
  Rust revision is pinned per mode in `parity-manifest.json`
  (`catalogRevisions`), next to the Go revision it stands for.
- **Divergence:** `D19.neutral-host-tool-names` is declared in
  `tools/parity/divergences.json`. Applied to the Rust side only, it rewrites
  the mapped names and the one field and replaces each pinned Rust revision
  with its Go pair. Any other difference stays a diff, and an unpinned
  revision is never mapped.
- **Compatibility:** Platform and recipes keep working through the aliases
  until cutover.
