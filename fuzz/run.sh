#!/usr/bin/env bash
# Run every fuzz target (or the ones named) for a bounded time each.
#   fuzz/run.sh SECONDS [TARGET...]
# Seeds come from fuzz/seeds; the working corpus and any crash land under
# fuzz/corpus and fuzz/artifacts (both ignored). Exit status is non-zero if
# any target found a crash, a timeout or an out-of-memory input.
set -u
cd "$(dirname "$0")/.."
seconds=${1:?usage: fuzz/run.sh SECONDS [TARGET...]}
shift
targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  mapfile -t targets < <(sed -n 's/^name = "\(.*\)"$/\1/p' fuzz/Cargo.toml | grep -v '^opute-host-agent-fuzz$')
fi
# Build once from the agent's own lock file, then run the built binaries: a
# workspace change during the run cannot rebuild a target halfway.
cp Cargo.lock fuzz/Cargo.lock
# cargo-fuzz defaults to the triple it was itself built for (a musl build in
# CI); build for the host toolchain's triple instead.
host=$(rustc -vV | sed -n 's/^host: //p')
cargo fuzz build -s none --release --target "$host" || exit 2
bin=fuzz/target/$host/release
status=0
mkdir -p fuzz/artifacts
for t in "${targets[@]}"; do
  mkdir -p "fuzz/corpus/$t" "fuzz/artifacts/$t"
  start=$(date +%s)
  log="fuzz/artifacts/$t.log"
  "$bin/$t" "fuzz/corpus/$t" "fuzz/seeds/$t" -artifact_prefix="fuzz/artifacts/$t/" \
    -max_total_time="$seconds" -timeout=20 -rss_limit_mb=4096 -print_final_stats=1 >"$log" 2>&1
  rc=$?
  runs=$(sed -n 's/^stat::number_of_executed_units: *//p' "$log" | tail -1)
  cov=$(grep -o 'cov: [0-9]*' "$log" | tail -1)
  echo "$t rc=$rc secs=$(( $(date +%s) - start )) runs=${runs:-?} ${cov:-cov: ?}"
  [ $rc -ne 0 ] && status=1
done
exit $status
