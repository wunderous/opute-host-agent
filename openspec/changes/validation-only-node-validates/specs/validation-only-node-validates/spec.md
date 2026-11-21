## ADDED Requirements

### Requirement: A validation-only node always evaluates its validation
When a plan node has a validation and no action, the Rust Host Agent SHALL evaluate the validation, polling within its declared timeout, whether or not a node it depends on was applied in the same run (owner decision D45). An applied dependency SHALL still make a node with an action run that action without a preflight.

#### Scenario: Waiting for an effect an earlier node started
- **WHEN** a node with no action depends on a node that was applied in the same run, and its validation's assertions hold
- **THEN** the node is satisfied and the run continues

#### Scenario: The awaited effect never appears
- **WHEN** the same node's assertions do not hold within its validation timeout
- **THEN** the node fails with the observed and expected values, as a failed validation does
