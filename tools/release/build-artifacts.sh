#!/usr/bin/env bash
# Build the release set under $DIST with Go's release names, compression and
# checksum manifest (Go's `make artifacts`, pinned in baseline/source-lock.json).
#
# - host-agent-linux-x64: Rust, linked against glibc 2.34 at most, the floor
#   Go's cgo-linked x64 release binary requires.
# - host-agent-linux-arm64: Rust, statically linked, as Go's arm64 binary is.
# - opute-provider-*: Go's provider executables, built unchanged from the
#   pinned Go tree with Go's own Makefile targets (M9 is deferred, D2).
# - host-agent-windows-x64.gz is not built: the Windows artifact is an owned
#   M10 gap.
#
# Both host-agent binaries are gzipped with `gzip -9 -kf`, as Go's are, and
# SHA256SUMS lists Go's files in Go's order, without the Windows archive.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
VERSION=${VERSION:-0.2.3}
DIST=${DIST:-$ROOT/dist}
GO_SRC=${GO_SRC:-$ROOT/.parity/go-src}
SETUP=${SETUP:-$ROOT/.parity/setup}
ZIG_DIR=${ZIG_DIR:-$SETUP/zig-x86_64-linux-0.15.2}
ZIGBUILD_DIR=${ZIGBUILD_DIR:-$SETUP/cargo-zigbuild/bin}
GO_BIN=${GO_BIN:-$SETUP/go/bin}
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target/dist}
GO_EXPORT=${GO_EXPORT:-$ROOT/target/dist/go-export}

export PATH=$ZIG_DIR:$ZIGBUILD_DIR:$GO_BIN:$PATH
command -v zig >/dev/null || { echo "zig not found in $ZIG_DIR (tools/release/setup-toolchain.sh)" >&2; exit 1; }
command -v cargo-zigbuild >/dev/null || { echo "cargo-zigbuild not found in $ZIGBUILD_DIR" >&2; exit 1; }

mkdir -p "$DIST"
build_agent() {
  local target=$1 name=$2
  OPUTE_HOST_AGENT_VERSION=$VERSION CARGO_TARGET_DIR=$TARGET_DIR \
    cargo zigbuild --release --locked --target "$target" --manifest-path "$ROOT/Cargo.toml"
  install -m 0775 "$TARGET_DIR/${target%%.*}/release/opute-host-agent" "$DIST/$name"
  gzip -9 -kf "$DIST/$name"
}
build_agent x86_64-unknown-linux-gnu.2.34 host-agent-linux-x64
build_agent aarch64-unknown-linux-musl host-agent-linux-arm64

# Go's providers, from a clean export of the pinned tree (the checkout is
# never written to), with Go's own targets and flags. Go records the build
# directory in the binary, so the export is always at the same path: the
# same tree and toolchain then give the same bytes.
providers=$GO_EXPORT
rm -rf "$providers"
mkdir -p "$providers"
git -C "$GO_SRC" archive HEAD | tar -x -C "$providers"
GOFLAGS=-mod=readonly GOPROXY=off GOTOOLCHAIN=local \
  make -s -C "$providers" build-provider-linux-x64 build-provider-cloudflare-linux-x64 \
  build-provider-tailscale-linux-x64 DIST=dist
for p in k3s cloudflare tailscale; do
  install -m 0775 "$providers/dist/opute-provider-$p-linux-x64" "$DIST/"
done

(cd "$DIST" && sha256sum host-agent-linux-x64 host-agent-linux-x64.gz host-agent-linux-arm64.gz \
  opute-provider-k3s-linux-x64 opute-provider-cloudflare-linux-x64 opute-provider-tailscale-linux-x64 > SHA256SUMS)
(cd "$DIST" && sha256sum -c --quiet SHA256SUMS)
echo "release set $VERSION in $DIST"
