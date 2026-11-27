# m32 inventory: local-LLM tools in 0.5.3

Source: the published 0.5.3 `tools/list` (host-agents
`site/context/release-catalog.json`, promoted from the published-canary
evidence, 138 tools) and `crates/host-agent/src/llmops.rs` and `builtin.rs` on
main `4799169f3`. Nothing here is inferred from a tool name alone.

## The 12 local-LLM tools are legacy wrappers

Every `*_local_llm_*` tool describes itself as "LEGACY compatibility wrapper
for a local runtime operation; prefer runtime-recipe.v1". The supported path
is a runtime recipe (`validate_runtime_recipe`, `run_runtime_recipe`) whose
plan nodes call host tools.

| Tool | Inputs (required) | What 0.5.3 does |
| --- | --- | --- |
| `check_local_llm_prerequisites` | none | Ollama only: architecture, CPU count, binary present, `opute-ollama.service` active, blockers |
| `install_local_llm_model` | none required; `runtime`, `modelRef`, `role`, `port`, `setDefault`, llama fields | Ollama: writes the user unit, starts it, `ollama pull`, warms and probes the model. llama-cpp: see below |
| `start_local_llm_runtime` / `stop_local_llm_runtime` | `runtime`, `runtimeId` | start or stop the user unit |
| `probe_local_llm` | `runtime`, `modelRef`, `includeChat` | Ollama: `/api/tags`, `/api/ps`, `/v1/models`; `chatReady` means the model is installed and listed, no completion is sent (`includeChat` only adds a hint) |
| `list_local_llm_models` | `runtime`, `includeChat` | the runtime's models |
| `configure_local_llm_model` | `modelRef`, `contextSize`/`numCtx` | Ollama context alias |
| `configure_local_llm_runtime` | none | always refuses: the Ollama runtime is host-wide and pinned |
| `remove_local_llm_model` | `modelRef` | always returns `removed: false, shared: true`: Ollama models are shared host state |
| `ensure_local_llm_server_binary` | `sourceUri`, `sourceSha256`, `sourceRevision` | builds llama-server with CUDA; refuses without `cmake`, `ninja`, `nvcc` |
| `ensure_local_llm_relay` / `remove_local_llm_relay` | session, listen, target, token, CIDRs | authenticated relay |
| `ensure_local_llm_k3s_proxy` / `remove_local_llm_k3s_proxy` | VM, ports, relay | publishes a relay into k3s |

## What a CPU-only host can run

- **llama.cpp is GPU-only by contract.** `validate_llama_config` requires a
  host CUDA build (`binarySource: host-build`, CUDA-linked), `--n-gpu-layers
  999` and `Q4_K_M`; `install_local_llm_model` refuses `binaryUri` and
  rejects a result that is not GPU-resident ("CPU fallback is rejected"). The
  CPU dev VM cannot use it; that path stays unverified.
- **Ollama is the default and works on CPU,** but the agent does not install
  Ollama. It finds the binary from `OPUTE_OLLAMA_BINARY` or `PATH` at the
  agent's start; otherwise every Ollama call reports "ollama binary is not
  installed".
- **Installing Ollama through the agent:** `ensure_host_artifact` (HTTPS,
  sha256, at most 4 GiB, 300 s request timeout) and `extract_host_archive`
  (`tar.zst` only, at most 8 GiB expanded). Ollama ships
  `ollama-linux-amd64.tar.zst`.
- **Cleanup:** `stop_local_llm_runtime` stops the unit, but no tool removes
  pulled Ollama models (by design, shared state), and the unit file and the
  extracted tree have no single remove tool. `remove_host_file` removes one
  regular file at a time.

## Pins chosen

| What | Pin |
| --- | --- |
| Ollama | v0.40.17 (2026-11-10), `ollama-linux-amd64.tar.zst`, 1,545,470,208 bytes, sha256 `1df9d6228bac5a22e420b242708c0d10e5b886db97bab99d446a41760f4e79af` (upstream `sha256sum.txt`) |
| Model | `hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M`; file `qwen2.5-0.5b-instruct-q4_k_m.gguf`, 491,400,032 bytes, sha256 `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db`, repo revision `9217f5db79a2` |

An Ollama model reference carries no revision, so the journey checks the
pulled blob's sha256 against the pin above.

## Questions the journey answers

1. Does the agent find an Ollama it installed under the user's home? The
   archive holds `bin/ollama` and `lib/ollama/`, so extracting it into
   `~/.local` gives `~/.local/bin/ollama`. The agent looks the binary up on
   each call, but on the `PATH` it started with; the npm launcher passes the
   user's environment. Ubuntu's `~/.profile` adds `~/.local/bin` only if it
   exists at login, so on a fresh user the agent needs a restart, or
   `OPUTE_OLLAMA_BINARY`, before it sees the binary. The journey measures
   which one applies, and the docs say so.
2. Does `ensure_host_artifact` finish a 1.5 GB download inside its 300 s
   request timeout on the dev VM?
3. Does `install_local_llm_model` (which warms the model) leave a runtime
   that answers a real `POST /v1/chat/completions` on CPU? No tool sends one,
   so the journey's client does, as a user's client would.
4. What is left behind after stop, and can the recipe remove it with
   published tools?

## Answers (journey e2e8.local-llm, run m32-r1)

1. The recipe writes `binaryPath` into `~/.config/opute/ollama-runtime.json`,
   which the agent reads on every call, so no restart and no `PATH` change is
   needed.
2. Not on 0.5.3: the download failed at 300 s (F1). With the fix it took 234 s.
3. Yes: the client's streaming request got "blue", with the first token after
   95 ms.
4. The archive, the unpacked Ollama, the config, the unit file and the model
   are left behind; no published tool removes them. The docs give the
   cleanup commands.

