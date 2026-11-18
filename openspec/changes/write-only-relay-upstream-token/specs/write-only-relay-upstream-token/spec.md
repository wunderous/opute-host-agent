## ADDED Requirements

### Requirement: The LLM relay's upstream token is published writeOnly
The served catalog SHALL publish `ensure_local_llm_relay.upstreamToken` as `writeOnly`, so the store never keeps it (owner decision D51).

#### Scenario: A relay is ensured
- **WHEN** `ensure_local_llm_relay` runs with an upstream token
- **THEN** the agent's state store does not hold the token
