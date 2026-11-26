#!/bin/bash
# Demo player: shows each line as typed at a prompt, then runs it for real.
# Lines starting with '#' are shown as comments and not run.
set -eo pipefail
PS="${DEMO_PROMPT:-\$ }"
type_out() { local s="$1" i; for ((i=0;i<${#s};i++)); do printf '%s' "${s:i:1}"; sleep 0.03; done; }
while IFS= read -r line; do
  printf '\033[1;32m%s\033[0m' "$PS"; sleep 0.4
  if [[ "$line" == \#* ]]; then printf '\033[2m'; type_out "$line"; printf '\033[0m\n'; sleep 0.6; continue; fi
  type_out "$line"; printf '\n'; sleep 0.3
  eval "$line"
  sleep 1.2
done < "$1"
# Off camera: keep the token for the client on the other machine; never printed.
if [ -n "$DEMO_SAVE_TOKEN" ]; then (umask 077; printf %s "$MCP_AUTH_TOKEN" > "$DEMO_SAVE_TOKEN"); fi
printf '\033[1;32m%s\033[0m' "$PS"; sleep 2
