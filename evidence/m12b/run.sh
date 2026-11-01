#!/usr/bin/env bash
# M12b evidence round: M12 validation 5 (soak). Run from the repository root
# on a clean tree, in three phases:
#   bash evidence/m12b/run.sh shakedown   # Go/Go and Go/Rust, 10 minutes each
#   (derive, review and commit tools/parity/soak-thresholds.json)
#   bash evidence/m12b/run.sh soak        # Go/Rust, 15 minutes
#   (close the soak gap in capture.py only if the soak passed; commit)
#   bash evidence/m12b/run.sh gates       # inventory recapture, gate reports
#
# Every step stays under 30 minutes (owner rule, 2026-11-01). The binaries
# and the harness are the ones the m12a round pinned, so its suites are not
# rerun: the m12b gate re-derives them from the committed m12a receipts and
# adds requireSoak. The soak phase refuses to start unless the thresholds
# are committed and unchanged, so they are declared before the soak.
#
# Twins boot from the pinned base golden in disposable qemu overlays and
# touch no shared Incus, Kubernetes, registry, tunnel or deploy state. A
# failing check is recorded, never retried or relabelled; a crashed round is
# retired under .parity/retired/ with its cause, never restarted silently.
set -u
cd "$(dirname "$0")/../.."
ROOT=$PWD
E=$ROOT/evidence/m12b
C=$E/checks
G=$ROOT/.parity/bin/go-reference
R=$ROOT/dist/host-agent-linux-x64
T=tools/parity/soak-thresholds.json
P="python3 -m parity"
PHASE=${1:?usage: run.sh shakedown|soak|gates}
export PATH=$ROOT/.parity/setup/go/bin:$ROOT/.parity/setup/node-v22.14.0-linux-x64/bin:$PATH
mkdir -p "$C"

tree() { { git status --porcelain --untracked-files=no; git diff HEAD; } | sha256sum; }
step() { local name=$1; shift; echo "== $name: $*" >&2; "$@" > "$C/$name.log" 2>&1; echo "$name exit=$?" >> "$C/exits.log"; }

case "$PHASE" in
shakedown)
  git rev-parse HEAD > "$C/head.shakedown"
  tree > "$C/tree.shakedown.before.sha256"
  : > "$C/exits.log"
  step harness make parity-test
  sha256sum "$G" "$R" > "$C/binaries.shakedown.sha256"
  # Go/Go sets the noise floor the thresholds are derived from; Go/Rust is
  # the trial run against them. Different seeds from the soak's.
  step shakedown-go-go bash -c "cd tools/parity && $P soak run --left $G --right $G --right-label go --seed 1201 --minutes 10 --out $E/shakedown-go-go"
  step shakedown-go-rust bash -c "cd tools/parity && $P soak run --left $G --right $R --seed 1202 --minutes 10 --out $E/shakedown-go-rust"
  step derive bash -c "cd tools/parity && $P soak derive --bundle $E/shakedown-go-go --source evidence/m12b/shakedown-go-go"
  step shakedown-evaluate bash -c "cd tools/parity && $P soak evaluate --bundle $E/shakedown-go-rust"
  tree > "$C/tree.shakedown.after.sha256"
  echo "shakedown complete: review and commit $T before the soak" >&2
  ;;
soak)
  git ls-files --error-unmatch "$T" >/dev/null 2>&1 && git diff --quiet HEAD -- "$T" \
    || { echo "$T must be committed, unchanged, before the soak" >&2; exit 1; }
  git rev-parse HEAD > "$C/head"
  tree > "$C/tree.before.sha256"
  sha256sum "$G" "$R" > "$C/binaries.sha256"
  sha256sum "$T" > "$C/thresholds.sha256"
  step soak bash -c "cd tools/parity && $P soak run --left $G --right $R --seed 1203 --minutes 15 --out $E/soak"
  step soak-evaluate bash -c "cd tools/parity && $P soak evaluate --bundle $E/soak"
  # D14 seam verifier and its dispatch canary.
  step seam python3 evidence/seam/capture.py m12b
  sha256sum "$G" "$R" > "$C/binaries.after.sha256"
  tree > "$C/tree.after.sha256"
  cp .parity/t2/golden.json "$E/t2-golden.json"
  echo "round complete" >&2
  ;;
gates)
  step capture make parity-capture
  for gate in m0 m1 m2 m3 m4 m5 m6 m8a m8b m8c m8d m8e m8f m10 m12a m12b cutover; do
    step verify-$gate bash -c "cd tools/parity && $P verify --gate $gate --report $E/verify-$gate.json"
  done
  echo "gates complete" >&2
  ;;
*) echo "usage: run.sh shakedown|soak|gates" >&2; exit 1 ;;
esac
