#!/usr/bin/env bash
# D25 evidence round: the parity rebase onto Go v0.2.4 (task 3.1), the Rust
# writer lock (3.2) and its refusal matrix (4.1-4.3). Run from the
# repository root on a clean tree, after `make release-toolchain` and
# `make artifacts`, in three phases:
#   bash evidence/d25/run.sh round   # every suite the gates re-derive
#   bash evidence/d25/run.sh soak    # M12 validation 5, 15 minutes
#   (close the writer-lock gap in capture.py only if 4.1-4.3 passed; commit)
#   bash evidence/d25/run.sh gates   # inventory recapture, gate reports
#
# The m12a round and the m12b soak, rerun against the v0.2.4 Go reference
# and the 0.2.4 release artifact. Owner rules (2026-11-01): every suite runs
# once, Go/Go self-validation three times, and no step runs over 30 minutes,
# so every step is cut off at 30 minutes and T2 is captured in shards by
# domain. The rehearsal's writer-lock drill refuses a second agent for
# Go/Go, Rust/Rust, Go then Rust and Rust then Go, and starts the next one
# after a SIGKILL; the clean-machine install journey refuses the user unit
# while the bootstrap unit serves the same instance.
#
# Twins boot from the pinned goldens in disposable qemu overlays and touch no
# shared Incus, Kubernetes, registry, tunnel or deploy state. A failing check
# is recorded, never retried or relabelled; a crashed round is retired under
# .parity/retired/ with its cause.
set -u
cd "$(dirname "$0")/../.."
ROOT=$PWD
E=$ROOT/evidence/d25
C=$E/checks
G=$ROOT/.parity/bin/go-reference
R=$ROOT/dist/host-agent-linux-x64
GR=$ROOT/.parity/releases/go
W=$ROOT/target/d25-work
GO=$ROOT/.parity/setup/go/bin/go
P="python3 -m parity"
PHASE=${1:?usage: run.sh round|soak|gates}
export PATH=$ROOT/.parity/setup/go/bin:$ROOT/.parity/setup/node-v22.14.0-linux-x64/bin:$PATH
command -v go >/dev/null && go version | grep -q "go1.25.4" || { echo "pinned Go toolchain missing" >&2; exit 1; }
mkdir -p "$C"

tree() { { git status --porcelain --untracked-files=no; git diff HEAD; } | sha256sum; }
# Exit 124 is a step cut off at 30 minutes.
step() { local name=$1; shift; echo "== $name: $*" >&2; timeout 1800 "$@" > "$C/$name.log" 2>&1; echo "$name exit=$?" >> "$C/exits.log"; }
t2ids() { python3 -c "import json,sys; from pathlib import Path
print(' '.join(s['id'] for d in sys.argv[1:] for s in json.loads(Path('tools/parity/scenarios-t2/'+d+'-domain.json').read_text())))" "$@"; }
T2_SHARDS="host:host incus:incus,serving,llm cluster:cluster,oci,postgres"

case "$PHASE" in
round)
  TMPROOT=${TMPDIR:-/tmp}
  [ "$(df --output=iavail "$TMPROOT" | tail -1)" -ge 200000 ] && [ "$(df --output=avail "$TMPROOT" | tail -1)" -ge 6000000 ] \
    || { echo "$TMPROOT needs 200k free inodes and 6 GB; clear old parity-a-*/parity-b-* sandboxes" >&2; exit 1; }
  git rev-parse HEAD > "$C/head"
  tree > "$C/tree.before.sha256"
  : > "$C/exits.log"

  # Standard checks, then the binaries every capture uses.
  step harness make parity-test
  step go-reference make go-reference
  step fmt cargo fmt --all -- --check
  step clippy cargo clippy --locked --all-targets -- -D warnings
  step workspace cargo test --locked
  # Both release sets as pinned, and both rebuilt to prove the binaries
  # reproduce (the archives differ only by the mtime gzip records).
  rm -rf "$W"; mkdir -p "$W"
  sha256sum dist/SHA256SUMS "$GR/SHA256SUMS" > "$C/releases.sha256"
  step rust-release bash -c "cd dist && sha256sum -c SHA256SUMS"
  step go-release bash -c "cd $GR && sha256sum -c SHA256SUMS"
  step rust-rebuild bash -c "DIST=$W/rust-rebuild CARGO_TARGET_DIR=$W/rust-target tools/release/build-artifacts.sh && cd $W/rust-rebuild && grep -v '\\.gz\$' $ROOT/dist/SHA256SUMS | sha256sum -c && cmp $ROOT/dist/host-agent-linux-arm64 host-agent-linux-arm64"
  step go-rebuild bash -c "cd tools/parity && $P dist release-go --out $W/go-rebuild && cd $W/go-rebuild && grep -v '\\.gz\$' $GR/SHA256SUMS | sha256sum -c && cmp $GR/host-agent-linux-arm64 host-agent-linux-arm64"
  sha256sum "$G" "$R" > "$C/binaries.sha256"
  step spec make spec-validate
  step catalog-source bash -c "cd tools/parity && $P catalog-source --go-src $ROOT/.parity/go-src --check"

  # Owning Go tests: the core, and the D25 lock's own tests.
  step go-owner-core bash -c "cd .parity/go-src && GOPROXY=off GOFLAGS=-mod=readonly GOTOOLCHAIN=local $GO test -count=1 ./internal/tools ./internal/catalog ./internal/plan ./internal/recipe ./internal/cordis ./internal/cordis/mcp"
  step go-owner-lock bash -c "cd .parity/go-src && GOPROXY=off GOFLAGS=-mod=readonly GOTOOLCHAIN=local $GO test -count=1 ./internal/instancelock && GOPROXY=off GOFLAGS=-mod=readonly GOTOOLCHAIN=local $GO test -count=1 -run 'InstanceLock|Instance|Locked|Check' ./internal/cli"

  # T1: shim-trace parity over every scenario.
  step go-vs-go bash -c "cd tools/parity && $P run --left go=$G --right go=$G --suite go-vs-go --repeat 3 --workers 4 --out $E/go-vs-go"
  step go-vs-rust bash -c "cd tools/parity && $P run --left go=$G --right rust=$R --suite go-vs-rust --repeat 1 --workers 4 --out $E/go-vs-rust"

  # Harness self-checks, contracts, client oracles.
  step canaries bash -c "cd tools/parity && $P canaries --go $G --go-src $ROOT/.parity/go-src --out $E/canaries.json"
  step rust-canaries bash -c "cd tools/parity && $P rust-canaries --rust $R --out $E/rust-canaries.json"
  for s in oauth-issuance standalone-read-only-gate reject-without-audit-writes refuse-before-operation-record evidence-redaction; do
    step contract-$s bash -c "cd tools/parity && $P contract --impl rust=$R --suite $s --out $E/contracts/$s"
  done
  step oracles bash -c "cd tools/parity && $P oracle --go-src $ROOT/.parity/go-src --binary go=$G --binary rust=$R --out $E/oracles.json"
  step oauth-sdk-client python3 tools/parity/oauth-sdk-client/run.py "$R" "$E/oauth-sdk-client"

  # M5 durable state, including the 16-field secret sweep.
  step m5-crash bash -c "cd tools/parity && $P storage-crash --go $G --rust $R --seeds 200 --out $E/m5/crash"
  step m5-secrets bash -c "cd tools/parity && $P secret-sweep --go $G --rust $R --out $E/m5/secrets"
  step m5-shape bash -c "cd tools/parity && $P shape --go $G --rust $R --repeat 1 --out $E/m5/shape"
  step m5-migration bash -c "cd tools/parity && $P migration --go $G --rust $R --out $E/m5/migration"
  step m5-cross-read bash -c "cd tools/parity && $P cross-read --go $G --rust $R --out $E/m5/cross-read"
  step m5-unknown-projection bash -c "cd tools/parity && $P unknown-projection --go $G --rust $R --out $E/m5/unknown-projection"

  # T2: effects in disposable twin VMs, one shard per group of domains.
  for shard in $T2_SHARDS; do
    ids=$(t2ids $(echo "${shard#*:}" | tr ',' ' '))
    step t2-go-vs-go-$name bash -c "cd tools/parity && $P t2 run --left go=$G --right go=$G --repeat 3 --workers 4 --out $E/t2-go-vs-go-$name $ids"
    step t2-go-vs-rust-$name bash -c "cd tools/parity && $P t2 run --left go=$G --right rust=$R --repeat 1 --workers 4 --out $E/t2-go-vs-rust-$name $ids"
  done

  # Go oracle in the sandbox: the baseline's own live Incus tests, inside
  # nested-Incus twins, against each binary and a negative control.
  step oracles-t2 bash -c "cd tools/parity && $P oracle --twin --go-src $ROOT/.parity/go-src --binary go=$G --binary rust=$R --out $E/oracles-t2.json"

  # M12a rehearsals and drills, with the D25 writer-lock drill.
  step rehearsal bash -c "cd tools/parity && $P rehearsal --go $G --rust $R --repeat 1 --out $E/rehearsal"

  # M10: release artifacts, clean machines (Ubuntu 24.04 and Debian 12),
  # upgrade and downgrade, the user/bootstrap refusal, the npm launcher and
  # the site.
  step dist bash -c "cd tools/parity && $P dist run --out $E/dist --work $W/dist"

  # D14 seam verifier and its dispatch canary.
  step seam python3 evidence/seam/capture.py d25

  sha256sum "$G" "$R" > "$C/binaries.after.sha256"
  sha256sum dist/SHA256SUMS "$GR/SHA256SUMS" > "$C/releases.after.sha256"
  tree > "$C/tree.after.sha256"
  for g in golden golden-incus golden-k3s golden-oci golden-postgres golden-debian; do
    cp .parity/t2/$g.json "$E/t2-$g.json"
  done
  echo "round complete" >&2
  ;;
soak)
  T=tools/parity/soak-thresholds.json
  git ls-files --error-unmatch "$T" >/dev/null 2>&1 && git diff --quiet HEAD -- "$T" \
    || { echo "$T must be committed, unchanged, before the soak" >&2; exit 1; }
  git rev-parse HEAD > "$C/head.soak"
  sha256sum "$G" "$R" > "$C/binaries.soak.sha256"
  sha256sum "$T" > "$C/thresholds.sha256"
  step soak bash -c "cd tools/parity && $P soak run --left $G --right $R --seed 1204 --minutes 15 --out $E/soak"
  step soak-evaluate bash -c "cd tools/parity && $P soak evaluate --bundle $E/soak"
  echo "soak complete" >&2
  ;;
gates)
  step capture make parity-capture
  for gate in m0 m1 m2 m3 m4 m5 m6 m8a m8b m8c m8d m8e m8f m10 m12a m12b d25 cutover; do
    step verify-$gate bash -c "cd tools/parity && $P verify --gate $gate --report $E/verify-$gate.json"
  done
  echo "gates complete" >&2
  ;;
*) echo "usage: run.sh round|soak|gates" >&2; exit 1 ;;
esac
