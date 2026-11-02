## ADDED Requirements

### Requirement: Rust provider executables match the pinned Go executables
Under decision D26, each ported provider (hostos, cloudflare, tailscale, k3s) SHALL be a Rust executable with the pinned Go executable's name, command line, environment, served MCP contract (`initialize`, `tools/list`, install manifest), call results, errors, callbacks into the Host Agent, external commands and exit codes. ollama is not ported (D15).

#### Scenario: Provider swap under the same core
- **WHEN** the Rust core runs a provider scenario once with the Go executable and once with the Rust executable
- **THEN** the core's observations, the provider boundary traffic and the shim traces are equal after the declared masks

#### Scenario: A Go provider defect
- **WHEN** a port finds Go provider behaviour that looks wrong
- **THEN** the Rust executable reproduces it, the finding is recorded, and any change goes to its own proposal

### Requirement: The release set builds without Go
The release build SHALL produce the full release set without a Go toolchain or the Go source once every shipped provider is ported. Until then it SHALL take the not-yet-ported Go provider executables from the pinned Go GitHub release, verified by that release's checksums, and SHALL NOT build them from the Go tree.

#### Scenario: Interim release build
- **WHEN** the release set is built before a provider is ported
- **THEN** that provider's executable is byte-identical to the pinned Go release's, and no Go toolchain runs

#### Scenario: A provider switches to Rust
- **WHEN** a provider's m9 gate passes
- **THEN** the release set ships its Rust executable under the same name and order, and the release checks and distribution journeys pass
