# README demo

`demo.gif` has two scenes, recorded with asciinema 3.2.1 at 100x16 and rendered
with agg 1.9.0 (`--font-size 16 --last-frame-duration 4`):

1. **Server** (`server-scene.txt`): a fresh Linux user starts the agent with
   `--allow-mutations` and checks its status.
2. **Laptop** (`client-scene.txt`): port 3014 is forwarded to the server over
   SSH, and `mcp.mjs` calls the agent: it lists the VM tools, reads the platform
   and capacity, writes one file, and shows that a wrong token is refused.

`mcp.mjs` is a scripted MCP client so each call is visible. With Claude, Cursor
or another MCP client, the assistant chooses and makes these calls.

`typed.sh` plays a scene: it types each line at the prompt, then runs it.

To record a scene again (Node 18+ and `jq` on the laptop):

```bash
mkdir -p bin && printf '#!/bin/sh\nexec node "%s/mcp.mjs" "$@"\n' "$PWD" > bin/mcp && chmod +x bin/mcp
PATH="$PWD/bin:$PATH" DEMO_PROMPT='you@laptop:~$ ' \
  asciinema rec --headless --window-size 100x16 -c './typed.sh client-scene.txt' client.cast
```

The token stays in `MCP_AUTH_TOKEN` and never appears on screen.
