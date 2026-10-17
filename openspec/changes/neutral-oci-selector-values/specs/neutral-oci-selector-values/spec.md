## ADDED Requirements

### Requirement: OCI selector values follow the neutral value map
The Rust Host Agent SHALL publish the selector values `podman`, `buildah` and `buildkit` as `container-engine`, `image-builder` and `build-daemon` (owner decision D23, extending D19). The map applies to the `builder` field of `ensure_oci_builder` and `build_and_push_oci_image`, and to the `runtime` field of `configure_oci_storage`, `inspect_container_storage` and `cleanup_container_storage`. `auto` SHALL stay `auto`. No tool name, field name, output key, effect, admission class, default or behaviour SHALL change.

#### Scenario: The catalog lists the neutral values
- **WHEN** a client lists tools in any serving mode
- **THEN** each mapped field's enum lists the published values in Go's order, and no mapped Go value appears

#### Scenario: Results publish the neutral value
- **WHEN** a mapped tool returns a structured result with a `builder` or `runtime` key whose value is a mapped Go value
- **THEN** that key carries the published value, at any depth, and every other value is returned as Go returns it

### Requirement: Go values remain accepted until cutover
A call or plan node that sends a mapped Go value SHALL be validated as the published value and SHALL reach the same Consumer with the same behaviour. Host state the domain persists SHALL keep Go's values.

#### Scenario: A Platform call with a Go value
- **WHEN** a client calls `ensure_oci_builder` with `builder: podman`
- **THEN** it behaves exactly as the same call with `builder: container-engine`

### Requirement: The difference stays declared and exact
`D19.neutral-host-tool-names` SHALL map these values back to Go's spelling on the Rust side only: in the mapped schemas' enums, and in `builder` and `runtime` values. Any other difference SHALL remain a diff.

#### Scenario: Neutral values compared with Go
- **WHEN** an OCI scenario runs Go against Rust
- **THEN** the divergence applies and is not stale, and the observations are identical
