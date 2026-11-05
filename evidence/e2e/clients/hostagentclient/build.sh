#!/usr/bin/env bash
# Build the e2e-1 hostagentclient offline: the pinned host-agents v0.2.4 tree
# (exported from c9f9e775 under .parity/e2e/go-src-v0.2.4), the module cache,
# GOPROXY=off, and a build cache under .parity. Prints the binary's sha256.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
PARITY=${PARITY:-$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir | xargs dirname)/.parity}
SRC=$PARITY/e2e/go-src-v0.2.4
OUT=$PARITY/e2e/inputs/hostagentclient
WORK=$PARITY/e2e/hostagentclient-build
rm -rf "$WORK" && mkdir -p "$WORK"
cp "$HERE/main.go" "$HERE/go.mod" "$WORK/"
cp "$SRC/go.sum" "$WORK/go.sum"
ln -s "$SRC" "$WORK/host-agents"
cd "$WORK"
GOTOOLCHAIN=local GOPROXY=off GOFLAGS=-mod=mod GOCACHE=$PARITY/e2e/gocache CGO_ENABLED=0 \
  "$PARITY/setup/go/bin/go" build -trimpath -ldflags=-buildid= -o "$OUT" .
sha256sum "$OUT"
