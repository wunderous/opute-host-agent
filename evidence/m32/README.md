# m32: a real local model through the agent

Goal: `.agents/goals/m32-local-llm.md`. Before m32, journey `e2e3.serving`
used a test-fixture runtime, so no real runtime and weights had ever run
through the agent.

- **Inventory:** [`inventory.md`](inventory.md). The local-LLM tools are
  legacy wrappers. On a CPU host, Ollama is the only runtime: the llama.cpp
  path is CUDA-only by contract.
- **Recipe:** [`docs/recipes/local-llm.recipe.yaml`](../../docs/recipes/local-llm.recipe.yaml)
  is a `runtime-recipe.v1` with `openai-chat.v1` activation. It runs four
  typed nodes:
  - `ensure_host_artifact` downloads Ollama v0.40.17, pinned by SHA-256;
  - `extract_host_archive` unpacks it;
  - `ensure_host_file` writes the agent's Ollama config with the binary path;
  - `install_local_llm_model` pulls and loads Qwen2.5-0.5B-Instruct Q4_K_M.
  - Each node has a validate step. The model's validation and the activation
    both send a streaming chat completion (`probe_openai_compatible_server`).
- **Journey:** `e2e8.local-llm` (`evidence/e2e/journeys/make_local_llm.py`),
  gate `e2e-local-llm`. It runs on the dev VM (8 vCPU, 16 GiB, no GPU), from
  the snapshot, against `rust-0.5.4-rc3` (commit `543b10b5a`, built with
  `VERSION=0.5.4`).

## What the journey found

| # | Finding | Result |
| --- | --- | --- |
| F1 | `ensure_host_artifact` allowed 300 s for the whole transfer. The 1.5 GB archive failed at about 5 MB/s: `read host artifact: timed out reading response` (draft 1, on 0.5.3). | Defect, fixed: `artifact-download-progress-timeout`. A download now fails after 300 s without progress, or one hour in all. Failing-first test `a_download_that_keeps_progressing_outlasts_the_idle_limit`. |
| F2 | Every node succeeded, then activation failed with `runtime probe returned an error`. The probe was admitted as a new call next to its own run and refused with `host_capacity_saturated` (draft 2, rc2). | Defect, fixed: `activation-probe-inherits-run-reservation`. Failing-first test `openai_chat_activation_probes_under_the_run`. |
| F3 | `stop_local_llm_runtime` leaves Ollama running: `stopped: false, shared: true`. | Recorded Go v0.2.4 behaviour for the shared runtime, not a defect. The journey stops the unit with `set_host_service_state`, and the docs say how. |
| F4 | `remove_local_llm_model` keeps the model: `removed: false, shared: true`. | Recorded behaviour (`recorded-session-and-llm-tools`). The docs give the cleanup commands. |
| F5 | `inspect_host_file` reads a whole file into memory to hash it: 1.5 GB for the archive's validate step. | Open observation, no change. |

Drafts 1 to 3 are retired under `.parity/retired/m32/`. Draft 3 failed only on
unused allowances, because the stopped unit leaves no processes or ports.

## Run m32-r1 (passes)

| Measured | Value |
| --- | --- |
| Download Ollama, 1,545,470,208 bytes | 234 s (about 6.6 MB/s) |
| Unpack, 80 entries, 2.5 GB | 6.5 s |
| Pull and load the model | 77 s |
| `run_runtime_recipe` to completed and activated | 320.7 s |
| Client streaming chat: first token, whole reply ("blue") | 95 ms, 109 ms |
| Disk after the run (MiB): archive, Ollama, `~/.ollama` | 1474, 2418, 469 |

Pinned hashes, checked in the guest:
- the archive: `1df9d622…79af`;
- the pulled GGUF blob: `74a4da8c…a9db`, the Hugging Face LFS digest of
  `qwen2.5-0.5b-instruct-q4_k_m.gguf`.

`make e2e-verify E2E_GATE=e2e-local-llm` re-derives the pass from the
committed receipts.

Not verified: a GPU host, arm64, and the published 0.5.3. 0.5.3 has F1 and F2,
so the recipe needs 0.5.4.
