# Run a local model

[`local-llm.recipe.yaml`](local-llm.recipe.yaml) puts a real model on a Linux
host in one recipe run, with no GPU needed. It:

1. downloads Ollama v0.40.17 from its GitHub release, checked against the
   release's SHA-256;
2. unpacks it under `~/.local/share/opute/ollama/` and points the agent at it
   (`~/.config/opute/ollama-runtime.json`);
3. starts Ollama as the user unit `opute-ollama.service`, pulls
   `hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M` and loads it;
4. sends a streaming chat completion, and records the runtime as the host's
   active `llm-serving` runtime only if that succeeds.

Afterwards any OpenAI-compatible client on the host can use
`http://127.0.0.1:11434/v1`.

![The recipe runs, then curl gets an answer](../demo/local-llm/local-llm.gif)

The waiting is sped up in this recording; [how it was made](../demo/local-llm/README.md).

## Requirements

- Agent 0.5.4 or later, started with `--allow-mutations` (see the
  [Quickstart](../../README.md#quickstart)), on x86-64 Linux with a systemd
  user session. 0.5.3 fails this recipe: its download gives up after 300
  seconds in all, and its activation check is refused.
- Disk: about 4.4 GB (1.5 GB archive, 2.4 GB unpacked Ollama, 0.5 GB model).
- Memory: the Ollama unit peaked at 1.4 GiB with the default model.
- Network: HTTPS to github.com and huggingface.co.

## Run it

Save the recipe on the host:

```bash
curl -fsSLo ~/local-llm.recipe.yaml https://raw.githubusercontent.com/wunderous/opute-host-agent/main/docs/recipes/local-llm.recipe.yaml
```

Then ask your assistant to run it:

> Run the runtime recipe at ~/local-llm.recipe.yaml with activate=true and
> wait for it to finish.

The assistant calls `run_runtime_recipe` with
`{"source": "/home/you/local-llm.recipe.yaml", "activate": true}` and then
polls `get_runtime_recipe_run` with the returned `runId`. To check the recipe
first without changing anything, use `validate_runtime_recipe` with the same
`source`.

## What to expect

Measured once on the project's test VM (8 vCPU, 16 GiB, no GPU, about 6.6
MB/s download), journey `e2e8.local-llm`, run `m32-r1`:

| Step | Time |
| --- | --- |
| Download Ollama (1.5 GB) | 234 s |
| Unpack it | 6.5 s |
| Pull and load the model (0.5 GB) | 77 s |
| The whole run, activation included | 5 min 20 s |
| First token of a reply, model loaded | 95 ms |

Download time depends on your link. The agent gives up on a download only
when no bytes arrive for five minutes, or after one hour in all.

The finished run reports `status: completed` and an `activeRuntime` with
`capability: llm-serving`, `servingContract: openai-chat.v1` and
`observation.streamingChatReady: true`.

## Use the model

```bash
curl -s http://127.0.0.1:11434/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model": "hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M", "messages": [{"role": "user", "content": "Say hello"}]}'
```

## Inputs

| Input | Default |
| --- | --- |
| `ollamaUri`, `ollamaSha256`, `ollamaVersion` | Ollama v0.40.17 `ollama-linux-amd64.tar.zst` and its SHA-256 |
| `modelRef` | `hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M` |

Pass other values in `inputs`, for example
`{"inputs": {"modelRef": "hf.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF:Q4_K_M"}}`.
The download pins Ollama by SHA-256. A model reference names no revision, so
check the pulled file yourself if that matters: the default model's GGUF has
SHA-256 `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db`
and lands at `~/.ollama/models/blobs/sha256-74a4da8c…`.

## Stop and clean up

The Ollama runtime is shared by every agent instance on the host, so
`stop_local_llm_runtime` leaves it running and says so (`stopped: false,
shared: true`), and `remove_local_llm_model` keeps the model (`removed:
false`). To stop it, ask your assistant to stop the user service
`opute-ollama.service` (`set_host_service_state`), or run
`systemctl --user stop opute-ollama.service`. To remove everything the recipe
added, Ollama's own key in `~/.ollama` included:

```bash
systemctl --user disable --now opute-ollama.service
rm -f ~/.config/systemd/user/opute-ollama.service ~/.config/opute/ollama-runtime.json
rm -rf ~/.ollama ~/.cache/opute/ollama ~/.local/share/opute/ollama
systemctl --user daemon-reload
```

## Limits

- The GPU path is unverified. Ollama uses a GPU when it finds one, but no
  release check runs on a GPU host. The agent's llama.cpp runtime
  (`ensure_local_llm_server_binary`) is CUDA-only by contract and refuses CPU
  hosts.
- The defaults are the `linux-amd64` archive. Ollama also publishes
  `ollama-linux-arm64.tar.zst`, which you can pass as `ollamaUri` with its
  SHA-256, but no release check runs on arm64.
- Rerunning the recipe with the same inputs replays the finished run; change
  an input to run it again.
