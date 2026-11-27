#!/bin/bash
# cli-operation-commands task 4.1, in the dev VM as root: a fresh user runs
# each operation command of the candidate binary on its own state. The
# agent that serves the VM's other work is not touched (another user, another
# instance). Prints a transcript; every step's exit code is shown.
set -eo pipefail
DIR=${1:?usage: guest-cli-run.sh DIR (holds host-agent, opute-provider-k3s, recipe.yaml, kubernetes.yaml)}
U=m31cli
H=/home/$U
cleanup() {
  pkill -u "$U" 2>/dev/null || true
  sleep 1
  userdel -r "$U" 2>/dev/null || true
}
trap cleanup EXIT
cleanup
useradd -m -s /bin/bash "$U"
install -o "$U" -m 0755 "$DIR/host-agent" "$H/host-agent"
install -o "$U" -m 0755 "$DIR/opute-provider-k3s" "$H/opute-provider-k3s"
install -o "$U" -m 0644 "$DIR/recipe.yaml" "$H/recipe.yaml"
# the provider's own activation recipe, beside its descriptor (recipes/kubernetes.yaml)
install -o "$U" -d "$H/recipes"
install -o "$U" -m 0644 "$DIR/kubernetes.yaml" "$H/recipes/kubernetes.yaml"
printf '{"schema":"opute-provider-plugin.v1","pluginId":"com.opute.k3s","version":"1.0.2","capabilities":[{"id":"opute.capability.kubernetes.v1","version":1}],"server":{"transport":"streamable_http","endpoint":"http://127.0.0.1:4331/mcp"}}\n' >"$H/provider.json"
printf 'OPUTE_CLOUDFLARED_TUNNEL_TOKEN=not-a-real-token\n' >"$H/tunnel.env"
chown "$U" "$H/provider.json" "$H/tunnel.env"
chmod 0600 "$H/tunnel.env"

as_user() {
  # Resource enforcement needs the workload slice the npm launcher creates;
  # this raw-binary run uses advisory enforcement instead.
  runuser -u "$U" -- env -i HOME="$H" PATH=/usr/bin:/bin USER="$U" \
    OPUTE_REMOTE_AGENT_ID=m31-cli-check OPUTE_HOST_RESOURCE_ENFORCEMENT=advisory "$@"
}
step() {
  local name=$1
  shift
  echo "### $name: $*"
  set +e
  as_user "$@" 2> >(grep -v 'msg=oauth' >&2) | tee "$H/last.out"
  local rc=${PIPESTATUS[0]}
  set -e
  echo "### $name exit=$rc"
  echo
}

cd "$H"
step version ./host-agent --version
step help ./host-agent help
step validate ./host-agent recipe validate --source "$H/recipe.yaml" --input word=vm
step applyRefused ./host-agent recipe apply --source "$H/recipe.yaml" --input word=vm
echo "### fileAfterRefusal: $(test -e "$H/cli-apply.txt" && echo present || echo absent)"
step apply ./host-agent recipe apply --allow-mutations --source "$H/recipe.yaml" --input word=vm
echo "### fileAfterApply: $(cat "$H/cli-apply.txt" 2>/dev/null || echo absent)"
RUN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["runId"])' "$H/last.out")
step status ./host-agent recipe status --run-id "$RUN"
# the provider, as an installed provider's process of this user
as_user bash -c 'OPUTE_PROVIDER_PORT=4331 nohup ./opute-provider-k3s >provider.log 2>&1 &'
for i in $(seq 60); do ss -ltnH 'sport = :4331' | grep -q . && break; sleep 0.5; done
echo "### providerListening: $(ss -ltnH 'sport = :4331' | wc -l)"
step installRefused ./host-agent provider install --source "$H/provider.json"
step install ./host-agent provider install --allow-mutations --activate --source "$H/provider.json"
step providerStatus ./host-agent provider status --provider com.opute.k3s
step validateProvider ./host-agent provider validate --provider com.opute.k3s
step reload ./host-agent provider reload --allow-mutations --activate --source "$H/provider.json"
step publicRefused ./host-agent public-mcp --binding-id m31 --endpoint https://m31.invalid/mcp \
  --local-target http://127.0.0.1:3004/mcp --token-file "$H/tunnel.env"
# beside a running agent on the same state, a command refuses
as_user bash -c 'MCP_AUTH_TOKEN=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d " \n") HOST_MCP_PORT=3399 nohup ./host-agent serve >agent.log 2>&1 &'
for i in $(seq 60); do ss -ltnH 'sport = :3399' | grep -q . && break; sleep 0.5; done
echo "### agentListening: $(ss -ltnH 'sport = :3399' | wc -l)"
step besideAgent ./host-agent provider status --provider com.opute.k3s
