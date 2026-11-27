# The activation probe runs under the run it validates

Status: **proposed (m32)**. A defect fix found by journey `e2e8.local-llm`. It
adds no tool, input, output field or default.

## Why

`run_runtime_recipe` with `activate: true` records a recipe's runtime as the
host's active one only after the host-owned validation flow for its serving
contract passes. For `openai-chat.v1` that flow calls
`probe_openai_compatible_server` with the recipe's bound `endpoint` and
`modelRef`.

Up to 0.5.4-rc1 the flow dispatched the probe as a new top-level call. The
run's own reservation was still held, so admission saw a second call competing
with the run and refused it:

    host_capacity_saturated: class=normal pressure=warning reason=declared resource cost exceeds effective host capacity

Activation then failed with `activation validation failed: runtime probe
returned an error`, after every plan node had succeeded. An `openai-chat.v1`
recipe could never be activated. Go ran the flow in the run's context, so the
probe inherited the run's reservation the way the run's nodes do. No journey
activated an `openai-chat.v1` recipe, so nothing caught it.

## What Changes

- The activation flow receives the run's context and dispatches the probe
  under it, so the probe is admitted as part of the run, as the run's nodes
  are.
- Nothing else changes: the same probe, the same bindings, the same
  `streamingChatReady` requirement and the same error texts.

## Impact

- `crates/host-agent/src/provider/runtime.rs` (`validate_activation`,
  `activation_flow`) and `crates/host-agent/src/plan_mcp.rs` (its caller).
- No catalog, schema or wire change.
- Test `openai_chat_activation_probes_under_the_run` in
  `tests/cli_operations.rs`; journey `e2e8.local-llm` activates the m32
  recipe on the dev VM.
