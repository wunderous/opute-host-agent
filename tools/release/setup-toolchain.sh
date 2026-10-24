#!/usr/bin/env bash
# Pinned release toolchain under .parity/setup: Zig (the linker that targets
# glibc 2.34 and static musl), cargo-zigbuild, and the arm64 musl standard
# library. Nothing is installed system-wide.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SETUP=${SETUP:-$ROOT/.parity/setup}
ZIG=zig-x86_64-linux-0.15.2
ZIG_SHA256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
ZIGBUILD_VERSION=0.23.4

mkdir -p "$SETUP"
cd "$SETUP"
if [ ! -x "$ZIG/zig" ]; then
  curl -sSfL -o "$ZIG.tar.xz" "https://ziglang.org/download/0.15.2/$ZIG.tar.xz"
  echo "$ZIG_SHA256  $ZIG.tar.xz" | sha256sum -c --quiet
  tar -xJf "$ZIG.tar.xz"
fi
if [ "$("$SETUP/cargo-zigbuild/bin/cargo-zigbuild" --version 2>/dev/null)" != "cargo-zigbuild $ZIGBUILD_VERSION" ]; then
  cargo install --locked --version "$ZIGBUILD_VERSION" --root "$SETUP/cargo-zigbuild" cargo-zigbuild
fi
rustup target add aarch64-unknown-linux-musl
