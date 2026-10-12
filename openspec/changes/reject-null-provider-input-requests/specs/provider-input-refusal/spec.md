## ADDED Requirements

### Requirement: Null provider input-request entries are refused without a panic
The Rust Host Agent SHALL refuse a provider tool result whose otherwise type-valid raw input-request map contains a null value with the decode error `nil InputRequest entry`. It SHALL NOT fulfill any input request from that refused result. The refusal SHALL leave the decoder available for later well-formed results. This requirement is a deliberate deviation from the pinned Go SDK decoder panic, approved by the owner as decision D17 (2026-10-12).

#### Scenario: Provider supplies a null input request
- **WHEN** a provider result contains `{"inputRequests":{"fixture-request":null}}`
- **THEN** Rust returns `nil InputRequest entry` without a panic and does not fulfill input requests from that result

#### Scenario: Well-formed result follows a refusal
- **WHEN** a valid roots, elicitation, or sampling input request follows a refused null entry
- **THEN** the same decoder accepts it under the pinned Go contract

### Requirement: The refusal difference remains explicit in evidence
Differential evidence SHALL retain the pinned Go panic and the Rust refusal as a named divergence. It SHALL NOT report exact Go parity for the null-entry scenario or waive another provider gate.

#### Scenario: Evidence includes the null-entry scenario
- **WHEN** a provider validation bundle includes null input-request entries
- **THEN** it records the Go panic and Rust refusal separately with source and binary provenance and cites decision D17
