# Provider-neutral instance tool names

## Why

D19 renamed the host-domain tools that named an implementation. The M8b
incus domain has one more tool name and one input field that do the same.
Under D14 only a Service Provider names the technology behind a capability,
so these names must become neutral before the domain is ported.

**Decided (owner, 2026-10-14), recorded as D21.** This change extends D19's
name map, with the same rules: neutral published names, and Go names kept as
hidden, deprecated aliases until cutover.

## What Changes

| Go name (alias) | Published Rust name |
| --- | --- |
| `reset_incus_stack` | `reset_virtualization_stack` |

The input field `provision_container.wslGpuLibs` becomes `hostGpuLibs`. No
other name, field, output key, effect, admission class, default or behaviour
changes. A description that names a renamed tool uses its new name; otherwise
descriptions keep Go's text.

An alias works exactly as under D19:

- It is absent from `tools/list` and `get_capability_catalog`, but
  `tools/call` accepts it.
- It takes Go's input schema, including `wslGpuLibs`.
- It reaches the same Consumer and returns the same result.
- Its operation and audit rows record the name that was called.

`provision_container` keeps its own name, so it has no alias. It accepts
`hostGpuLibs` and, for compatibility, Go's `wslGpuLibs`. The published
schema declares only `hostGpuLibs`.

## Capabilities

### New Capabilities

- `neutral-instance-tool-names`: the incus-domain entries in the one name
  map, and their evidence.

### Modified Capabilities

None. Go is unchanged.

## Impact

- **Name map:** the entries join `crates/host-agent/catalog/neutral-names.json`,
  the one map the catalog and the parity divergence already read.
- **Catalog revisions:** every Rust catalog revision changes again. Each is
  re-pinned in `parity-manifest.json` with its Go pair, as D19 requires.
- **Divergence:** `D19.neutral-host-tool-names` reads the map, so it covers
  the new entries without a second rule. Any other difference stays a diff.
- **Compatibility:** Platform and recipes keep working through the alias and
  the accepted Go field until cutover.
