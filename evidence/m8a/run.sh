#!/usr/bin/env bash
# M8a (host domain) evidence round. Run from the repository root on a clean
# tree:  bash evidence/m8a/run.sh
#
# Every check writes its raw log under evidence/m8a/checks; captures go to
# evidence/m8a/<suite>. A failing check is recorded, never retried or
# relabelled: the round continues so every result is on record, and
# verify.py decides. Nothing here edits tracked files except the outputs
# under evidence/m8a and evidence/seam/m8a.
set -u
cd "$(dirname "$0")/../.."
ROOT=$PWD
E=$ROOT/evidence/m8a
C=$E/checks
G=$ROOT/.parity/bin/go-reference
R=$ROOT/target/release/opute-host-agent
GO=$ROOT/.parity/setup/go/bin/go
P="python3 -m parity"
export PATH=$ROOT/.parity/setup/node-v22.14.0-linux-x64/bin:$PATH
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
step release cargo build --release --locked
sha256sum "$G" "$R" > "$C/binaries.sha256"
step spec make spec-validate
step catalog-source bash -c "cd tools/parity && $P catalog-source --go-src $ROOT/.parity/go-src --check"

# Owning Go tests: the host domain and its runtime seams (x5), and the core.
step go-owner-host bash -c "cd .parity/go-src && GOPROXY=off GOFLAGS=-mod=readonly GOTOOLCHAIN=local $GO test -count=5 -v ./internal/domain/host/... ./internal/hostruntime/... ./internal/resource/..."
step go-owner-core bash -c "cd .parity/go-src && GOPROXY=off GOFLAGS=-mod=readonly GOTOOLCHAIN=local $GO test -count=1 ./internal/tools ./internal/catalog ./internal/plan ./internal/recipe ./internal/cordis ./internal/cordis/mcp"

# T1: shim-trace parity over every scenario.
step go-vs-go bash -c "cd tools/parity && $P run --left go=$G --right go=$G --suite go-vs-go --repeat 20 --workers 4 --out $E/go-vs-go"
step go-vs-rust bash -c "cd tools/parity && $P run --left go=$G --right rust=$R --suite go-vs-rust --repeat 5 --workers 4 --out $E/go-vs-rust"

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
step m5-shape bash -c "cd tools/parity && $P shape --go $G --rust $R --repeat 5 --out $E/m5/shape"
step m5-migration bash -c "cd tools/parity && $P migration --go $G --rust $R --out $E/m5/migration"
step m5-cross-read bash -c "cd tools/parity && $P cross-read --go $G --rust $R --out $E/m5/cross-read"
step m5-unknown-projection bash -c "cd tools/parity && $P unknown-projection --go $G --rust $R --out $E/m5/unknown-projection"

# T2: effects in disposable twin VMs.
step t2-go-vs-go bash -c "cd tools/parity && $P t2 run --left go=$G --right go=$G --repeat 20 --workers 4 --out $E/t2-go-vs-go"
step t2-go-vs-rust bash -c "cd tools/parity && $P t2 run --left go=$G --right rust=$R --repeat 5 --workers 4 --out $E/t2-go-vs-rust"

# D14 seam verifier and its dispatch canary.
step seam python3 evidence/seam/capture.py m8a

sha256sum "$G" "$R" > "$C/binaries.after.sha256"
tree > "$C/tree.after.sha256"
cp .parity/t2/golden.json "$E/t2-golden.json"
echo "round complete" >&2
