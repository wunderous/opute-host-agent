#!/usr/bin/env bash
# Build the release set under $DIST with Go's release names, compression and
# checksum manifest (Go's `make artifacts`, pinned in baseline/source-lock.json).
#
# - host-agent-linux-x64: Rust, linked against glibc 2.34 at most, the floor
#   Go's cgo-linked x64 release binary requires.
# - host-agent-linux-arm64: Rust, statically linked, as Go's arm64 binary is.
# - opute-provider-*: Rust, the ported providers (D26, M9), statically
#   linked for x64 musl, as Go's provider executables are. No Go toolchain
#   or Go tree.
# - host-agent-windows-x64.gz is not built: the Windows artifact is an owned
#   M10 gap.
#
# Both host-agent binaries are gzipped with `gzip -9 -kf`, as Go's are, and
# SHA256SUMS lists Go's files in Go's order, without the Windows archive.
#
# m16: a CycloneDX SBOM of the agent's dependency graph (Linux targets, no
# build-only dependencies) is written beside the set as sbom.cdx.json, with
# its own sbom.cdx.json.sha256; SHA256SUMS keeps Go's file list.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
VERSION=${VERSION:-0.2.4}
DIST=${DIST:-$ROOT/dist}
SETUP=${SETUP:-$ROOT/.parity/setup}
ZIG_DIR=${ZIG_DIR:-$SETUP/zig-x86_64-linux-0.15.2}
ZIGBUILD_DIR=${ZIGBUILD_DIR:-$SETUP/cargo-zigbuild/bin}
CYCLONEDX_DIR=${CYCLONEDX_DIR:-$SETUP/cargo-cyclonedx/bin}
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target/dist}

export PATH=$ZIG_DIR:$ZIGBUILD_DIR:$CYCLONEDX_DIR:$PATH
command -v zig >/dev/null || { echo "zig not found in $ZIG_DIR (tools/release/setup-toolchain.sh)" >&2; exit 1; }
command -v cargo-zigbuild >/dev/null || { echo "cargo-zigbuild not found in $ZIGBUILD_DIR" >&2; exit 1; }
command -v cargo-cyclonedx >/dev/null || { echo "cargo-cyclonedx not found in $CYCLONEDX_DIR" >&2; exit 1; }

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

providers=(k3s cloudflare tailscale)
CARGO_TARGET_DIR=$TARGET_DIR cargo zigbuild --release --locked --target x86_64-unknown-linux-musl \
  --manifest-path "$ROOT/Cargo.toml" "${providers[@]/#/--bin=opute-provider-}"
for p in "${providers[@]}"; do
  install -m 0775 "$TARGET_DIR/x86_64-unknown-linux-musl/release/opute-provider-$p" "$DIST/opute-provider-$p-linux-x64"
done

(cd "$DIST" && sha256sum host-agent-linux-x64 host-agent-linux-x64.gz host-agent-linux-arm64.gz \
  opute-provider-k3s-linux-x64 opute-provider-cloudflare-linux-x64 opute-provider-tailscale-linux-x64 > SHA256SUMS)
(cd "$DIST" && sha256sum -c --quiet SHA256SUMS)
(cd "$ROOT/crates/host-agent" && cargo cyclonedx -f json --spec-version 1.5 --no-build-deps \
  --target x86_64-unknown-linux-gnu --override-filename sbom.cdx >/dev/null)
mv "$ROOT/crates/host-agent/sbom.cdx.json" "$DIST/sbom.cdx.json"
(cd "$DIST" && sha256sum sbom.cdx.json > sbom.cdx.json.sha256)
echo "release set $VERSION in $DIST"
