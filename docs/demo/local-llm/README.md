# Local-model demo

`local-llm.gif` shows [Run a local model](../../recipes/local-llm.md) on a
Linux host with no GPU:
1. download the recipe from `main`;
2. run it with `run_runtime_recipe` and `activate: true`;
3. wait for it;
4. ask the model a question with `curl`.

It was recorded with asciinema 3.2.1 at 100x16 and rendered with agg 1.9.0
(`--font-size 16 --last-frame-duration 4 --idle-time-limit 2`):
- **The wait is compressed.** `--idle-time-limit 2` cuts every pause to two
  seconds, so the roughly five-minute wait for the Ollama download and the
  model load takes a few seconds here. Commands, output and order are as
  recorded.
- **The client is scripted:** `../mcp.mjs`, as in the README demo. With an
  MCP client such as Claude, the assistant makes these calls.
- **The agent is a release candidate.** A fresh user started it with the
  published npm launcher (`npx @opute/host-agent@0.5.3 start --background
  --allow-mutations`) and `OPUTE_HOST_AGENT_BINARY` set to the
  `rust-0.5.4-rc3` binary, whose files were later published unchanged as
  0.5.4.
- **The host** is an Incus container on the project's test VM:
  6 CPUs, 6 GiB, no GPU.

`evidence/m32/guest-demo.sh` makes the recording and then runs the rest of
the docs page as written. The token stays in the user's environment and
never appears on screen.
