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
| F5 | `inspect_host_file` reads a whole file into memory to hash it: 1.5 GB for the archive's validate step. `remove_host_file` hashed the same way. | Fixed after 0.5.4: both stream the file through the hash in 64 KiB chunks. Failing-first test `inspect_hashes_a_file_larger_than_the_memory_limit` hashes a 384 MiB file in a child limited to 256 MiB of address space. Results and errors are unchanged, except that a failure while reading now names Go's `read` operation instead of `open`. |

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

## The docs, as written, and the demo

[`docs-run.txt`](docs-run.txt) is from `guest-demo.sh`, run after PR #86
merged (`ebde82356`). It ran in an Incus container on the dev VM (6 CPUs, 6 GiB),
so minimal.opute.io kept serving.

A fresh user started the agent with the published launcher
(`npx @opute/host-agent@0.5.3 start --background --allow-mutations`), with
`OPUTE_HOST_AGENT_BINARY` set to the `rust-0.5.4-rc3` binary. Then, in this
order:
- **The demo scene** (`docs/demo/local-llm/scene.txt`) downloaded the recipe
  from `main` with the docs' URL and ran it with `activate: true`. The run
  reported `completed` with `llm-serving` active. The model answered "Paris".
- **"Use the model"** ran verbatim and got a completion.
- **Memory:** the unit's `MemoryPeak` was 1,461,592,064 bytes (1.4 GiB). An
  earlier dry run measured 1,475,788,800.
- **"Stop and clean up"** ran verbatim. Nothing it lists was left, the unit
  was inactive and port 11434 was closed.

The GIF is `docs/demo/local-llm/local-llm.gif`: 53.3 s, 315 KB. The session
took 414 s, and pauses are cut to 2 s. The cast holds no token.

Not verified: a GPU host, arm64, and the published 0.5.3. 0.5.3 has F1 and F2,
so the recipe needs 0.5.4.
