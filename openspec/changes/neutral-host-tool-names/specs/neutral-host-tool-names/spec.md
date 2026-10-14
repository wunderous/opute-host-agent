## ADDED Requirements

### Requirement: Host tools are published under provider-neutral names
The Rust Host Agent SHALL publish the eight mapped host-domain tools under their neutral names (owner decision D19). Those names SHALL be used in `tools/list`, in `get_capability_catalog`, and wherever a published descriptor names another tool. `install_virtualization_stack` SHALL take the field `packageName` where Go's `install_incus_stack` takes `incusPackage`. No other name, field, output key, effect, admission class, default or behaviour SHALL change.

#### Scenario: The catalog lists neutral names only
- **WHEN** a client lists tools in any serving mode
- **THEN** the mapped tools appear under their neutral names, no Go name from the map appears, and the tool count equals Go's

#### Scenario: A descriptor names the neutral tool
- **WHEN** a client reads the `compact_host_vm_disk` descriptor
- **THEN** its description refers to `shutdown_host_vm_subsystem`, not `shutdown_wsl`

### Requirement: Go names remain hidden, deprecated aliases until cutover
Each Go name in the map SHALL be accepted by `tools/call` as an alias of its neutral tool. An alias SHALL take Go's input schema, reach the same Consumer, and return the same result and errors. Its operation and audit rows SHALL record the name that was called. An alias SHALL NOT appear in `tools/list` or `get_capability_catalog`. Removing the aliases SHALL need its own change at cutover.

#### Scenario: A Platform call by the Go name
- **WHEN** a client calls `install_incus_stack` with `incusPackage`
- **THEN** the call is admitted and executed exactly as `install_virtualization_stack` with `packageName`, and its operation row names `install_incus_stack`

#### Scenario: An unknown name stays unknown
- **WHEN** a client calls a name that is neither a published tool nor a mapped alias
- **THEN** Rust answers Go's unknown-tool error

### Requirement: The difference is declared, exact and pinned
The parity suite SHALL declare `D19.neutral-host-tool-names`. Applied to the Rust side only, it SHALL rewrite each mapped neutral name to its Go name, `packageName` to `incusPackage` on `install_virtualization_stack`, and each Rust catalog revision pinned in `parity-manifest.json` to its paired Go revision. Any other difference, or any revision without a pinned pair, SHALL remain a diff.

#### Scenario: A renamed catalog compared with Go
- **WHEN** a catalog scenario runs Go against Rust after the rename
- **THEN** the divergence is applied and not stale, and the normalized observations are identical

#### Scenario: An unpinned revision fails
- **WHEN** Rust publishes a catalog revision that has no pinned Go pair
- **THEN** the comparison reports the revision as a difference
