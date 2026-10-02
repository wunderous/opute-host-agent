## MODIFIED Requirements

### Requirement: Typed target and effect admission

The Host Agent SHALL resolve tenant-scoped canonical resource URIs and resource
kinds before execution, apply the same authorization, approval, effect, and
mutation gates as Go, and pass tool-specific arguments to the owning capability
without guessing or rewriting IDs. `vm:` and `container:` targets SHALL remain
distinct. No success result may be invented from a rejected or unknown target.
The standalone mutation gate of the Rust agent SHALL follow
`standalone-read-only-gate` (decision D10): with mutations disabled it runs
only tools whose effect is `read`.

#### Scenario: Mutation gate remains closed
- **WHEN** a standalone client calls a tool whose effect is not `read` while the explicit mutation gate is disabled
- **THEN** Rust denies the call before side effects with a compatible typed error

#### Scenario: Runtime kind differs
- **WHEN** an operation requires a container but the canonical target resolves to a virtual machine
- **THEN** Rust rejects the mismatch before side effects instead of coercing the URI or choosing another executor
