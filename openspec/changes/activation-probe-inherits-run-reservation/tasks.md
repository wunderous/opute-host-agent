## 1. Tests first

- [x] 1.1 `openai_chat_activation_probes_under_the_run` (`tests/cli_operations.rs`): a one-node recipe activated against an in-process OpenAI-compatible endpoint. It fails on 0.5.4-rc1 with `activation validation failed: runtime probe returned an error`.

## 2. Change

- [x] 2.1 `validate_activation` takes the run's `RunCtx`, and the `openai-chat.v1` flow dispatches the probe with it.

## 3. Evidence

- [x] 3.1 Journey `e2e8.local-llm` activates the m32 recipe on the dev VM on `rust-0.5.4-rc3` (run `m32-r1`, gate `e2e-local-llm`).
