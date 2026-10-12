## ADDED Requirements

### Requirement: Failed provider publication preserves prior family owners
The Host Agent SHALL validate a candidate's schemas and prepare its family displacement before committing publication changes. A failed candidate publication SHALL retain the previous registry, published catalog, executable bindings, remembered manifests, and MCP projection. This is a deliberate change from the pinned Go ordering, approved as owner decision D16.

#### Scenario: Invalid schema competes for an existing capability family
- **WHEN** provider Alpha owns a family and provider Beta attempts publication in that family with an invalid schema
- **THEN** publication fails and Alpha's operation remains in every prior publication projection

#### Scenario: Unrelated publication follows a failed replacement
- **WHEN** provider Gamma publishes another family after Beta's rejected replacement
- **THEN** Alpha remains available and Gamma's successful publication does not expose delayed displacement from the failed attempt

### Requirement: Successful publication retains existing exclusivity and affinity
Successful candidate publication SHALL retain the existing per-family exclusivity rule. Calls accepted before replacement SHALL retain their original executable and generation bindings. Failed activation rollback SHALL restore every family owner affected by that activation.

#### Scenario: Replacement succeeds with an accepted predecessor call
- **WHEN** Beta successfully replaces Alpha's family while a call accepted by Alpha remains in flight
- **THEN** new calls use Beta and the accepted call completes through Alpha's original generation
