#!/bin/bash
# m32 demo and docs run, in the dev VM as root. A fresh user starts the agent
# with the published npm launcher, pointed at the release candidate's binary,
# records docs/demo/local-llm/scene.txt, then runs the rest of
# docs/recipes/local-llm.md as written and removes the user. The token is
# generated here, kept in the user's environment and never printed.
set -eo pipefail
DIR=${1:?usage: guest-demo.sh DIR (holds node.tar.gz, mcp.mjs, typed.sh, scene.txt, asciinema)}
U=llm
H=/home/$U
BIN=${BIN:-/opt/opute-e2e/releases/rust/v0.5.4/host-agent-linux-x64}
cleanup() {
  if id "$U" >/dev/null 2>&1; then
    runuser -u "$U" -- env XDG_RUNTIME_DIR=/run/user/$(id -u "$U") systemctl --user stop opute-ollama.service 2>/dev/null || true
    loginctl disable-linger "$U" 2>/dev/null || true
    pkill -u "$U" 2>/dev/null || true
    sleep 2
    userdel -r "$U" 2>/dev/null || true
  fi
  rm -rf /opt/m32
}
trap cleanup EXIT
cleanup
mkdir -p /opt/m32/bin
tar -xzf "$DIR/node.tar.gz" -C /opt/m32
install -m 0755 "$DIR/asciinema" /opt/m32/bin/asciinema
install -m 0644 "$DIR/mcp.mjs" /opt/m32/mcp.mjs
# where the demo user can run it
install -m 0755 "$BIN" /opt/m32/host-agent
BIN=/opt/m32/host-agent
printf '#!/bin/sh\nexec /opt/m32/node/bin/node /opt/m32/mcp.mjs "$@"\n' > /opt/m32/bin/mcp
chmod 0755 /opt/m32/bin/mcp
useradd -m -s /bin/bash "$U"
install -o "$U" -m 0755 "$DIR/typed.sh" "$H/typed.sh"
install -o "$U" -m 0644 "$DIR/scene.txt" "$H/scene.txt"
loginctl enable-linger "$U"
for i in $(seq 100); do [ -S /run/user/$(id -u "$U")/bus ] && break; sleep 0.1; done

as_user() {
  runuser -u "$U" -- env -i HOME="$H" USER="$U" LOGNAME="$U" SHELL=/bin/bash TERM=xterm-256color \
    XDG_RUNTIME_DIR=/run/user/$(id -u "$U") PATH=/opt/m32/bin:/opt/m32/node/bin:/usr/bin:/bin \
    HOST_MCP_PORT=3024 MCP_URL=http://127.0.0.1:3024/mcp OPUTE_HOST_AGENT_BINARY="$BIN" \
    bash -c "cd ~ && . ~/.agent-env && $1"
}
# the agent's identity and token, as the README quickstart makes them
runuser -u "$U" -- bash -c 'umask 077; printf "export OPUTE_REMOTE_AGENT_ID=local-%s\nexport MCP_AUTH_TOKEN=%s\n" "$(openssl rand -hex 8)" "$(openssl rand -hex 32)" > ~/.agent-env'
echo "### start"
as_user 'npx -y @opute/host-agent@0.5.3 start --background --allow-mutations >/dev/null'
status=$(as_user 'npx -y @opute/host-agent@0.5.3 status' | grep -v 'npm notice')
echo "$status"
jq -e '.running and .healthy' <<<"$status" >/dev/null

echo "### record"
as_user "DEMO_PROMPT='you@host:~\$ ' asciinema rec --headless --window-size 100x16 -c './typed.sh scene.txt' local-llm.cast >/dev/null 2>&1"
install -m 0644 "$H/local-llm.cast" "$DIR/local-llm.cast"
grep -c '"o"' "$DIR/local-llm.cast" | sed 's/^/cast events: /'

echo "### docs: Use the model (as written)"
as_user "curl -s http://127.0.0.1:11434/v1/chat/completions \\
  -H 'Content-Type: application/json' \\
  -d '{\"model\": \"hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M\", \"messages\": [{\"role\": \"user\", \"content\": \"Say hello\"}]}' | jq -c '{model, content: .choices[0].message.content}'"

echo "### memory of the runtime unit"
as_user 'systemctl --user show -p MemoryPeak -p MemoryCurrent opute-ollama.service'

echo "### docs: Stop and clean up (as written)"
as_user 'systemctl --user disable --now opute-ollama.service
rm -f ~/.config/systemd/user/opute-ollama.service ~/.config/opute/ollama-runtime.json
rm -rf ~/.ollama ~/.cache/opute/ollama ~/.local/share/opute/ollama
systemctl --user daemon-reload'
as_user 'for p in .ollama .cache/opute/ollama .local/share/opute/ollama .config/opute/ollama-runtime.json .config/systemd/user/opute-ollama.service; do test -e ~/$p && echo "left $p"; done; systemctl --user is-active opute-ollama.service || true; ss -ltnH "sport = :11434" | wc -l'
as_user 'npx -y @opute/host-agent@0.5.3 stop >/dev/null || true'
echo "### done"
