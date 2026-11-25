#!/bin/bash
# D56 check as a fresh user: the packed 0.5.1 launcher with the 0.5.0 binary.
# MODE=session (login user) or MODE=nosession or MODE=readonly. Never prints the token.
set -eo pipefail
export PATH=/opt/node/bin:$PATH
export OPUTE_HOST_AGENT_BINARY=/opt/d56/opute-host-agent
export OPUTE_REMOTE_AGENT_ID="local-$(openssl rand -hex 8)"
export MCP_AUTH_TOKEN="$(openssl rand -hex 32)"
L() { npx -y --package=/opt/d56/opute-host-agent-0.5.1.tgz opute-host-agent "$@"; }
SLICE=~/.config/systemd/user/opute-workload.slice
META='"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"d56","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}'
write() {
  curl -sS http://127.0.0.1:3014/mcp -H 'Accept: application/json, text/event-stream' -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $MCP_AUTH_TOKEN" -H 'MCP-Protocol-Version: 2026-07-28' -H 'Mcp-Method: tools/call' -H 'Mcp-Name: ensure_host_file' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"ensure_host_file\",\"arguments\":{\"path\":\"quickstart/hello.txt\",\"content\":\"$1\"},$META}}" \
    | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print("ensure_host_file:", "isError" if r.get("isError") else "ok", r["content"][0]["text"][:120])'
}

case "$MODE" in
session)
  echo "slice before: $(test -e $SLICE && echo present || echo absent)"
  L start --background --allow-mutations 2>&1
  echo "slice after: $(test -e $SLICE && echo present || echo absent), mode $(stat -c %a $SLICE)"
  write one
  L stop
  echo "-- second start keeps the slice"
  sum=$(sha256sum $SLICE)
  L start --background --allow-mutations 2>&1
  [ "$(sha256sum $SLICE)" = "$sum" ] && echo "slice unchanged"
  write two
  L stop
  ;;
nosession)
  set +e
  L start --background --allow-mutations > /tmp/d56-ns.out 2>&1; rc=$?
  set -e
  echo "exit $rc"; head -3 /tmp/d56-ns.out
  echo "slice: $(test -e $SLICE && echo present || echo absent); health: $(curl -s -o /dev/null -w %{http_code} http://127.0.0.1:3014/health || true)"
  ;;
readonly)
  L start --background 2>&1
  echo "slice: $(test -e $SLICE && echo present || echo absent)"
  L stop
  ;;
esac
