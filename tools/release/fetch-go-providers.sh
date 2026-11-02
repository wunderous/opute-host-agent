#!/usr/bin/env bash
# Fetch the pinned Go release's provider executables and SHA256SUMS (the
# release named by sourceRelease in baseline/source-lock.json) into
# .parity/releases/go-published, and check them against the source lock.
# Run once before tools/release/build-artifacts.sh; needs read access to
# wunderous/host-agents through gh.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT=${GO_RELEASE:-$ROOT/.parity/releases/go-published}
lock() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$ROOT/baseline/source-lock.json" "$1"; }
tag=$(lock sourceRelease)
sums=$(lock sourceReleaseSha256Sums); sums=${sums#sha256:}
mkdir -p "$OUT"
gh release download "$tag" --repo wunderous/host-agents --dir "$OUT" --clobber -p SHA256SUMS \
  -p opute-provider-k3s-linux-x64 -p opute-provider-cloudflare-linux-x64 -p opute-provider-tailscale-linux-x64
echo "$sums  $OUT/SHA256SUMS" | sha256sum -c --quiet
for p in k3s cloudflare tailscale; do
  (cd "$OUT" && grep " opute-provider-$p-linux-x64\$" SHA256SUMS | sha256sum -c --quiet)
done
chmod 0775 "$OUT"/opute-provider-*-linux-x64
echo "Go $tag providers in $OUT"
