# Provider-neutral OCI selector values

## Why

The M8d OCI domain's nine tool names are already neutral (`oci`,
`container_storage` and `build_context` name open formats). Provider names
appear only as values that select an implementation:

- `builder`: `auto`, `podman`, `buildah`, `buildkit`
- `runtime`: `auto`, `podman`

Under D14 only a Service Provider names the technology behind a capability.

**Decided (owner, 2026-10-17), recorded as D23.** Asked to keep Go's values
or publish neutral ones, the owner chose neutral values, then approved this
map as proposed. It extends the one D19 name map with a value section, with
the same rules as D19, D21 and D22: neutral published values, and Go values
kept as hidden, deprecated aliases until cutover.

## What Changes

| Go value (alias) | Published value |
| --- | --- |
| `auto` | `auto` (unchanged) |
| `podman` | `container-engine` |
| `buildah` | `image-builder` |
| `buildkit` | `build-daemon` |

The map applies to two fields:

- **`builder`:** in `ensure_oci_builder` and `build_and_push_oci_image`.
- **`runtime`:** in `configure_oci_storage`, `inspect_container_storage` and
  `cleanup_container_storage`.

For those five tools:

- **Published input schemas:** each field's `enum` lists the published values
  in Go's order.
- **Inputs:** a Go value is accepted as its published value, in calls and in
  plan nodes. It is validated as the published value and reaches the
  Consumer as Go's value, so the domain behaves exactly as Go.
- **Results:** a `builder` or `runtime` key whose value is a mapped Go value
  publishes the mapped value, at any depth of the structured result. For
  example, `ensure_oci_builder` returns `builder` and `runtime`, and
  `cleanup_container_storage` returns `runtime` in `before` and `after`. An
  unmapped value, such as `docker`, is returned as Go returns it.
- **Unchanged:** tool names, field names, output keys, effects, admission
  classes and defaults.
- **Text:** descriptions and error text keep Go's wording. An example is
  "builder must be one of auto, podman, buildah, or buildkit".
- **Host state:** the storage policy file, which is host state that a Go
  agent must still read, keeps Go's values.

## Capabilities

### New Capabilities

- `neutral-oci-selector-values`: the value section of the one name map, and
  its evidence.

### Modified Capabilities

None. Go is unchanged.

## Impact

- **Name map:** the value section joins
  `crates/host-agent/catalog/neutral-names.json`.
- **Catalog revisions:** every Rust catalog revision changes again. Each is
  re-pinned in `parity-manifest.json` with its Go pair, as D19 requires.
- **Divergence:** `D19.neutral-host-tool-names` reads the map. On the Rust
  side only, it rewrites:
  - the five schemas' enum values;
  - `builder` and `runtime` values in observations.

  Any other difference stays a diff.
- **Compatibility:** Platform and recipes that send Go's values keep working
  until cutover.
