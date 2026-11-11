## ADDED Requirements

### Requirement: open_assistant_session negotiates the session contract
The Rust Host Agent SHALL answer `open_assistant_session` with `contractVersion: "assistant-session.v1"`, the caller's `sessionId`, the current `catalogRevision`, the provider id and the active `tenantId` when `supportedContractVersions` contains `assistant-session.v1`, `catalogRevision` is absent or equals the current revision, and `tenantId` is absent or equals the active tenant. Otherwise it SHALL end the call with a typed tool error naming the mismatch. It writes nothing.

#### Scenario: Compatible client
- **WHEN** a client offers `["assistant-session.v1"]` and the current revision
- **THEN** the result names `assistant-session.v1` and that revision

#### Scenario: Stale revision
- **WHEN** a client names a catalog revision that is not current
- **THEN** the call ends with a typed error naming the requested and current revisions

### Requirement: The legacy LLM wrappers act on the shared Ollama runtime
`remove_local_llm_model` SHALL remove nothing and return `removed: false`, `purged: false`, `shared: true` and a reason saying Ollama model artifacts are shared host state. `configure_local_llm_runtime` SHALL change nothing and end with a typed tool error saying the Ollama runtime configuration is host-wide and pinned. Neither SHALL stop, start or reconfigure the runtime.

#### Scenario: Remove a shared model
- **WHEN** a client calls `remove_local_llm_model` for a model the shared runtime serves
- **THEN** the result is `removed: false, shared: true`, and the model is still listed by `list_local_llm_models`

#### Scenario: Configure the runtime
- **WHEN** a client calls `configure_local_llm_runtime`
- **THEN** the call ends with the typed error, and the runtime's settings are unchanged
