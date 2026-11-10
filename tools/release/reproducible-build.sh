#!/usr/bin/env bash
# Locked, offline, reproducible build check (m16).
#
# 1. `cargo fetch --locked`: every dependency comes from Cargo.lock.
# 2. Two release builds with `--offline --locked`, each in its own fresh
#    target directory, with the checkout and cargo home remapped so no local
#    path is embedded.
# 3. The two agent binaries and provider executables must be byte-identical.
#
# The release set itself is built by build-artifacts.sh with zig; this check
# proves the Rust build is deterministic and network-free on the host
# toolchain. Writes a receipt to $OUT (default target/reproducible/receipt.json).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=${WORK:-$ROOT/target/reproducible}
OUT=${OUT:-$WORK/receipt.json}
CARGO_HOME_DIR=${CARGO_HOME:-$HOME/.cargo}
bins=(opute-host-agent opute-provider-k3s opute-provider-cloudflare opute-provider-tailscale opute-provider-hostos)

cargo fetch --locked --manifest-path "$ROOT/Cargo.toml"
rm -rf "$WORK"
for run in a b; do
  RUSTFLAGS="--remap-path-prefix=$ROOT=/build --remap-path-prefix=$CARGO_HOME_DIR=/cargo --remap-path-prefix=$WORK/$run=/target" \
  CARGO_TARGET_DIR="$WORK/$run" \
    cargo build --release --locked --offline --manifest-path "$ROOT/Cargo.toml" "${bins[@]/#/--bin=}"
done

status=0
entries=()
for b in "${bins[@]}"; do
  a=$(sha256sum "$WORK/a/release/$b" | cut -d' ' -f1)
  c=$(sha256sum "$WORK/b/release/$b" | cut -d' ' -f1)
  same=true
  [ "$a" = "$c" ] || { same=false; status=1; echo "differs: $b" >&2; }
  entries+=("{\"binary\": \"$b\", \"buildA\": \"$a\", \"buildB\": \"$c\", \"identical\": $same}")
done
commit=$(git -C "$ROOT" rev-parse HEAD)
dirty=$([ -z "$(git -C "$ROOT" status --porcelain -- crates Cargo.toml Cargo.lock)" ] && echo false || echo true)
{
  echo "{\"schema\": \"m16-reproducible-build.v1\", \"commit\": \"$commit\", \"sourceDirty\": $dirty,"
  echo " \"rustc\": \"$(rustc --version)\", \"offline\": true, \"locked\": true,"
  printf ' "binaries": [%s],\n' "$(IFS=,; echo "${entries[*]}")"
  echo " \"pass\": $([ $status -eq 0 ] && echo true || echo false)}"
} > "$OUT"
cat "$OUT"
exit $status
