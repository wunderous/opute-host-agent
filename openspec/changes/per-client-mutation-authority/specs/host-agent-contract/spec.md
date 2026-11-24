## MODIFIED Requirements

### Requirement: Typed target and effect admission

The Host Agent SHALL resolve tenant-scoped canonical resource URIs and resource
kinds before execution, apply the recorded authorization, approval, effect,
and mutation gates as amended by the accepted decisions, and pass
tool-specific arguments to the owning capability without guessing or
rewriting IDs. `vm:` and `container:` targets SHALL remain distinct. No
success result may be invented from a rejected or unknown target. A tool
counts as `read` as `standalone-read-only-gate` (decision D10) decides; a
call to any other tool SHALL be admitted only when the caller's own grant
includes mutations, as `per-client-mutation-authority` (decision D55)
decides.

#### Scenario: Mutation gate remains closed
- **WHEN** a caller whose grant does not include mutations calls a tool whose effect is not `read`
- **THEN** Rust denies the call before side effects with an error naming the grant that would admit it

#### Scenario: Runtime kind differs
- **WHEN** an operation requires a container but the canonical target resolves to a virtual machine
- **THEN** Rust rejects the mismatch before side effects instead of coercing the URI or choosing another executor
