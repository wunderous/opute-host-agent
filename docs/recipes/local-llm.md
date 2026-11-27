# Run a local model

[`local-llm.recipe.yaml`](local-llm.recipe.yaml) puts a real model on a Linux
host in one recipe run, with no GPU needed. It:

1. downloads Ollama {OLLAMA} from its GitHub release, checked against the
   release's SHA-256;
2. unpacks it under `~/.local/share/opute/ollama/` and points the agent at it
   (`~/.config/opute/ollama-runtime.json`);
3. starts Ollama as the user unit `opute-ollama.service`, pulls
   `hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M` and loads it;
4. sends a streaming chat completion, and records the runtime as the host's
   active `llm-serving` runtime only if that succeeds.

Afterwards any OpenAI-compatible client on the host can use
`http://127.0.0.1:11434/v1`.

## Requirements

- Agent 0.5.3 or later, started with `--allow-mutations` (see the
  [Quickstart](../../README.md#quickstart)), on x86-64 Linux with a systemd
  user session.
- Disk: {DISK}.
- Network: HTTPS to github.com and huggingface.co.
- Memory: {MEMORY}.

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

{EXPECT}

## Use the model

```bash
curl -s http://127.0.0.1:11434/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model": "hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M", "messages": [{"role": "user", "content": "Say hello"}]}'
```

## Inputs

| Input | Default |
| --- | --- |
| `ollamaUri`, `ollamaSha256`, `ollamaVersion` | Ollama {OLLAMA} `ollama-linux-amd64.tar.zst` and its SHA-256 |
| `modelRef` | `hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M` |

Pass other values in `inputs`, for example
`{"inputs": {"modelRef": "hf.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF:Q4_K_M"}}`.
The download pins Ollama by SHA-256. A model reference names no revision, so
check the pulled file yourself if that matters: the default model's GGUF has
SHA-256 `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db`
and lands at `~/.ollama/models/blobs/sha256-74a4da8c…`.

## Stop and clean up

`stop_local_llm_runtime` with `{"runtime": "ollama"}` stops the unit.
`remove_local_llm_model` keeps the model on purpose: Ollama models are shared
host state. To remove everything the recipe added:

```bash
systemctl --user disable --now opute-ollama.service
rm -f ~/.config/systemd/user/opute-ollama.service ~/.config/opute/ollama-runtime.json
rm -rf ~/.ollama ~/.cache/opute/ollama ~/.local/share/opute/ollama
systemctl --user daemon-reload
```

## Limits

{LIMITS}
