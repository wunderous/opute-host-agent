#!/usr/bin/env bash
# Build the pinned Go Host Agent reference binary reproducibly.
#
# Reads baseline/source-lock.json, checks out exactly that commit, refuses to
# build if the commit or tree differs, and prints a JSON provenance record.
# The build uses -trimpath, CGO_ENABLED=0 and fixed ldflags so the binary hash
# depends only on the source tree and the Go toolchain version.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
lock="$repo_root/baseline/source-lock.json"
work="${PARITY_GO_WORKDIR:-$repo_root/.parity/go-src}"
out="${PARITY_GO_OUT:-$repo_root/.parity/bin/go-reference}"

read_lock() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$lock" "$1"; }
url="$(read_lock sourceRepository)"
commit="$(read_lock sourceCommit)"
tree="$(read_lock sourceTree)"
version="$(read_lock publishedPackage)"; version="${version##*@}"
# The standard library is part of the observable HTTP contract. Go 1.26,
# for example, changes ServeMux path-cleaning redirects from 301 to 307.
# Do not silently build the pinned reference with the machine's newer Go.
export GOTOOLCHAIN="$(read_lock goToolchain)"

if [ ! -d "$work/.git" ]; then
  mkdir -p "$work"
  git -C "$work" init -q
  git -C "$work" remote add origin "$url"
fi
if ! git -C "$work" cat-file -e "$commit^{commit}" 2>/dev/null; then
  GIT_LFS_SKIP_SMUDGE=1 git -C "$work" fetch -q --depth 1 origin "$commit"
fi
git -C "$work" checkout -q --detach "$commit"
git -C "$work" reset -q --hard "$commit"
git -C "$work" clean -qfdx

actual_commit="$(git -C "$work" rev-parse HEAD)"
actual_tree="$(git -C "$work" rev-parse 'HEAD^{tree}')"
if [ "$actual_commit" != "$commit" ] || [ "$actual_tree" != "$tree" ]; then
  echo "source lock mismatch: commit $actual_commit tree $actual_tree" >&2
  exit 1
fi

mkdir -p "$(dirname "$out")"
(
  cd "$work"
  CGO_ENABLED=0 go build -trimpath -buildvcs=false \
    -ldflags="-s -w -buildid= -X github.com/wunderous/host-agents/internal/version.Version=$version" \
    -o "$out" ./cmd/opute-host-agent
)

python3 - "$out" "$commit" "$tree" "$version" <<'PY'
import hashlib, json, subprocess, sys
path, commit, tree, version = sys.argv[1:]
digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
go = subprocess.run(["go", "env", "GOVERSION"], capture_output=True, text=True, check=True).stdout.strip()
print(json.dumps({"binary": path, "binarySha256": digest, "sourceCommit": commit,
                  "sourceTree": tree, "version": version, "goVersion": go}, indent=2))
PY
