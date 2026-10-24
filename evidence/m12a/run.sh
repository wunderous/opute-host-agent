#!/usr/bin/env bash
# M12a (M12 without Platform) evidence round. Run from the repository root on
# a clean tree, after `make release-toolchain` and `make artifacts`:
#   bash evidence/m12a/run.sh
#
# The M10 round, rerun against the rebuilt release artifact
# (dist/host-agent-linux-x64), plus the cutover rehearsals (parity
# rehearsal): forward and rollback on checksummed copies of representative
# state stopped with a task in flight, and the stop drills. Every suite
# repeats 3 times. M11 (staging Platform) is the open dependency.
#
# Every check writes its raw log under evidence/m12a/checks; captures go to
# evidence/m12a/<suite>. A failing check is recorded, never retried or
# relabelled: the round continues so every result is on record, and
# verify.py decides. Nothing here edits tracked files except the outputs
# under evidence/m12a and evidence/seam/m12a.
set -u
cd "$(dirname "$0")/../.."
ROOT=$PWD
E=$ROOT/evidence/m12a
C=$E/checks
G=$ROOT/.parity/bin/go-reference
R=$ROOT/dist/host-agent-linux-x64
GR=$ROOT/.parity/releases/go
W=$ROOT/target/m12a-work
GO=$ROOT/.parity/setup/go/bin/go
P="python3 -m parity"
export PATH=$ROOT/.parity/setup/go/bin:$ROOT/.parity/setup/node-v22.14.0-linux-x64/bin:$PATH
command -v go >/dev/null && go version | grep -q "go1.25.4" || { echo "pinned Go toolchain missing" >&2; exit 1; }
# Each capture leaves its sandboxes under TMPDIR (about 15k directories a
# round); a full tmpfs fails captures mid-round, so require room up front.
TMPROOT=${TMPDIR:-/tmp}
[ "$(df --output=iavail "$TMPROOT" | tail -1)" -ge 200000 ] && [ "$(df --output=avail "$TMPROOT" | tail -1)" -ge 6000000 ] \
  || { echo "$TMPROOT needs 200k free inodes and 6 GB; clear old parity-a-*/parity-b-* sandboxes" >&2; exit 1; }
mkdir -p "$C"

tree() { { git status --porcelain --untracked-files=no; git diff HEAD; } | sha256sum; }
step() { local name=$1; shift; echo "== $name: $*" >&2; "$@" > "$C/$name.log" 2>&1; echo "$name exit=$?" >> "$C/exits.log"; }

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

# Owning Go tests: the core (the launcher's and the site's own tests run
# inside the distribution round, from a clean export of the pinned tree).
step go-owner-core bash -c "cd .parity/go-src && GOPROXY=off GOFLAGS=-mod=readonly GOTOOLCHAIN=local $GO test -count=1 ./internal/tools ./internal/catalog ./internal/plan ./internal/recipe ./internal/cordis ./internal/cordis/mcp"

# T1: shim-trace parity over every scenario.
step go-vs-go bash -c "cd tools/parity && $P run --left go=$G --right go=$G --suite go-vs-go --repeat 3 --workers 4 --out $E/go-vs-go"
step go-vs-rust bash -c "cd tools/parity && $P run --left go=$G --right rust=$R --suite go-vs-rust --repeat 3 --workers 4 --out $E/go-vs-rust"

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
step m5-shape bash -c "cd tools/parity && $P shape --go $G --rust $R --repeat 3 --out $E/m5/shape"
step m5-migration bash -c "cd tools/parity && $P migration --go $G --rust $R --out $E/m5/migration"
step m5-cross-read bash -c "cd tools/parity && $P cross-read --go $G --rust $R --out $E/m5/cross-read"
step m5-unknown-projection bash -c "cd tools/parity && $P unknown-projection --go $G --rust $R --out $E/m5/unknown-projection"

# T2: effects in disposable twin VMs: host-, incus-, cluster-, oci- and
# postgres-, llm- and serving-domain scenarios (booted from the base, nested-Incus, k3s, OCI and
# PostgreSQL goldens; the
# harness's memory budget bounds how many twin pairs run at once).
step t2-go-vs-go bash -c "cd tools/parity && $P t2 run --left go=$G --right go=$G --repeat 3 --workers 4 --out $E/t2-go-vs-go"
step t2-go-vs-rust bash -c "cd tools/parity && $P t2 run --left go=$G --right rust=$R --repeat 3 --workers 4 --out $E/t2-go-vs-rust"

# Go oracle in the sandbox: the baseline's own live Incus tests, inside
# nested-Incus twins, against each binary and a negative control.
step oracles-t2 bash -c "cd tools/parity && $P oracle --twin --go-src $ROOT/.parity/go-src --binary go=$G --binary rust=$R --out $E/oracles-t2.json"

# M12a: forward and rollback rehearsals on state copies, and the stop drills.
step rehearsal bash -c "cd tools/parity && $P rehearsal --go $G --rust $R --repeat 3 --out $E/rehearsal"

# M10: release artifacts, clean machines (Ubuntu 24.04 and Debian 12),
# upgrade and downgrade, the npm launcher and the site.
step dist bash -c "cd tools/parity && $P dist run --out $E/dist --work $W/dist"

# D14 seam verifier and its dispatch canary.
step seam python3 evidence/seam/capture.py m12a

sha256sum "$G" "$R" > "$C/binaries.after.sha256"
sha256sum dist/SHA256SUMS "$GR/SHA256SUMS" > "$C/releases.after.sha256"
tree > "$C/tree.after.sha256"
cp .parity/t2/golden.json "$E/t2-golden.json"
cp .parity/t2/golden-incus.json "$E/t2-golden-incus.json"
cp .parity/t2/golden-k3s.json "$E/t2-golden-k3s.json"
cp .parity/t2/golden-oci.json "$E/t2-golden-oci.json"
cp .parity/t2/golden-postgres.json "$E/t2-golden-postgres.json"
cp .parity/t2/golden-debian.json "$E/t2-golden-debian.json"
echo "round complete" >&2
