# open_assistant_session and the two legacy LLM tools, as recorded

Status: no new decision: these follow the frozen v0.2.4 record (D27).
Goal m15.

## Why

The catalog publishes `open_assistant_session`, `remove_local_llm_model` and
`configure_local_llm_runtime`, and Rust answers `not_implemented`. Go v0.2.4
handled all three; the spec did not state what they do.

## What Changes

The spec states the recorded behaviour, and Rust implements it on the D14
seam:

- `open_assistant_session` negotiates the `assistant-session.v1` contract
  and binds the client to the current catalog revision and tenant.
- Neither LLM tool publishes a `runtime` input, so the runtime is always the
  shared Ollama runtime: `remove_local_llm_model` reports the shared model
  retained, and `configure_local_llm_runtime` refuses with a typed error.

## Impact

`crates/host-agent/src` (built-in services); `e2e3.serving` gains both LLM
tools.
