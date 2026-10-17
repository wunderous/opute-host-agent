## ADDED Requirements

### Requirement: Instance tools follow the neutral name map
The Rust Host Agent SHALL publish `reset_incus_stack` as `reset_virtualization_stack`, and `provision_container`'s field `wslGpuLibs` as `hostGpuLibs` (owner decision D21, extending D19). No other name, field, output key, effect, admission class, default or behaviour SHALL change.

#### Scenario: The catalog lists the neutral name
- **WHEN** a client lists tools in any serving mode
- **THEN** `reset_virtualization_stack` appears, `reset_incus_stack` does not, and `provision_container` declares `hostGpuLibs` and not `wslGpuLibs`

### Requirement: Go names and fields remain accepted until cutover
`tools/call` SHALL accept `reset_incus_stack` as a hidden alias with Go's input schema, reaching the same Consumer and recording the called name. `provision_container` SHALL accept `wslGpuLibs` with Go's meaning. Neither SHALL appear in a published catalog.

#### Scenario: A Platform call with Go's field
- **WHEN** a client calls `provision_container` with `wslGpuLibs: true`
- **THEN** it behaves exactly as the same call with `hostGpuLibs: true`

### Requirement: The difference stays declared and exact
`D19.neutral-host-tool-names` SHALL map these names, the field and each pinned Rust revision back to Go's spelling on the Rust side only. Any other difference SHALL remain a diff.

#### Scenario: Renamed catalog compared with Go
- **WHEN** a catalog scenario runs Go against Rust
- **THEN** the divergence applies and is not stale, and the observations are identical
