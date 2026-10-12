#!/usr/bin/env bash
# Build the five pinned Go provider executables reproducibly, unchanged.
#
# Uses the checkout that build-go-reference.sh pinned (it must already be at
# the locked commit and tree, and clean) and the same flags: -trimpath,
# CGO_ENABLED=0, no VCS stamp and an empty build id, so each hash depends only
# on the source tree and the locked Go toolchain. Prints a JSON provenance
# record.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
lock="$repo_root/baseline/source-lock.json"
work="${PARITY_GO_WORKDIR:-$repo_root/.parity/go-src}"
out="${PARITY_PROVIDERS_OUT:-$repo_root/.parity/bin/providers}"

read_lock() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$lock" "$1"; }
export GOTOOLCHAIN="$(read_lock goToolchain)"
commit="$(read_lock sourceCommit)"
tree="$(read_lock sourceTree)"
if [ "$(git -C "$work" rev-parse HEAD)" != "$commit" ] || [ "$(git -C "$work" rev-parse 'HEAD^{tree}')" != "$tree" ] \
    || [ -n "$(git -C "$work" status --porcelain)" ]; then
  echo "pinned Go checkout is not at the source lock; run build-go-reference.sh first" >&2
  exit 1
fi

mkdir -p "$out"
for plugin in platform/hostos llm/ollama kubernetes/k3s tunneling/cloudflare tunneling/tailscale; do
  name="opute-provider-$(basename "$plugin")"
  (
    cd "$work/plugins/$plugin"
    CGO_ENABLED=0 go build -trimpath -buildvcs=false -ldflags="-s -w -buildid=" -o "$out/$name" "./cmd/$name"
  )
done
if [ -n "$(git -C "$work" status --porcelain)" ]; then
  echo "building the providers changed the pinned checkout" >&2
  exit 1
fi

python3 - "$out" "$commit" "$tree" <<'PY'
import hashlib, json, pathlib, subprocess, sys
out, commit, tree = sys.argv[1:]
go = subprocess.run(["go", "env", "GOVERSION"], capture_output=True, text=True, check=True).stdout.strip()
binaries = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(pathlib.Path(out).iterdir())}
print(json.dumps({"binaries": binaries, "sourceCommit": commit, "sourceTree": tree, "goVersion": go}, indent=2))
PY
