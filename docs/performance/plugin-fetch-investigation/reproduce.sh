#!/usr/bin/env bash
# Replays the plugin-fetch investigation's measurements at the pinned revision.
#
# Usage: bash reproduce.sh <mode>
#   route-read   the scout's frozen bench cells (10 s profile loop each, two reps)
#   plugin       the bun client fixture (parse / rank / pack), three processes
#   profile-read perf record of read/1000-rows/narrow-scope (cycles:u)
#   scout        stage attribution + typed encoder + pushdown, several fixtures
#   pushdown-join the pushdown scan with the memory-domain join
#   concurrency  closed-loop 1/4/8-thread route vs pushdown at 1,000 rows
#   typed-ab     applies evidence/typed-read-body.patch, builds the bench,
#                runs three interleaved rounds against the baseline binary,
#                reverts the patch
#   parity       bun vs Rust lowercase samples and ranking over a dumped body
#   checks       the daemon route tests that cover the patched paths
#
# ROOT must be a clean checkout at the pinned revision. Every step stays under
# 60 s except builds; a cold dependency build needs a warm shared target dir.
set -euo pipefail
HERE=$(cd -- "$(dirname -- "$0")" && pwd)
ROOT=${ROOT:-$(cd -- "$HERE/../../.." && pwd)}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/local/home/ahrav/scratch/eidnara/target}
MODE=${1:?choose route-read, plugin, profile-read, scout, pushdown-join, concurrency, typed-ab, parity, or checks}
cd "$ROOT"
test "$(git rev-parse HEAD)" = 8e0491225a7292ef077c675d44b94f94a24041d3 || echo "warning: HEAD is not the pinned revision" >&2
OUT=$(mktemp -d "$HERE/replay.XXXXXX")
CREATED=()
cleanup() { for p in "${CREATED[@]}"; do rm -f -- "$p"; done; }
trap cleanup EXIT
run() {
  local name=$1; shift
  printf '%q ' "$@" >> "$OUT/commands.log"; printf '\n' >> "$OUT/commands.log"
  "$@" > "$OUT/$name.log" 2>&1 || { tail -40 "$OUT/$name.log"; return 1; }
}
install_scout() {
  local dest="crates/daemon/examples/read_route_scout.rs"
  test ! -e "$dest"
  mkdir -p "$(dirname "$dest")"
  cp "$HERE/sources/read_route_scout.rs" "$dest"
  CREATED+=("$dest")
  run build-scout cargo build --release --locked -p daemon --example read_route_scout
  SCOUT="$CARGO_TARGET_DIR/release/examples/read_route_scout"
}
build_routes() {
  run build-routes cargo bench --locked -p daemon --bench kernel_routes --features test-support --no-run --message-format=json
  ROUTE=$(python3 - "$OUT/build-routes.log" <<'PY'
import json,sys
for line in open(sys.argv[1]):
    try: x=json.loads(line)
    except ValueError: continue
    if x.get('reason')=='compiler-artifact' and x.get('target',{}).get('name')=='kernel_routes' and x.get('executable'):
        print(x['executable'])
PY
)
  test -n "$ROUTE"
}
profile_cells() {
  local bin=$1 tag=$2
  for rep in a b; do
    for cell in read/10-rows/narrow-scope read/1000-rows/narrow-scope read/1000-rows/wide-scope; do
      run "$tag-${cell//\//-}-$rep" timeout 55s env EIDNARA_KERNEL_ROUTES_PROFILE="$cell" "$bin"
    done
  done
}
case "$MODE" in
route-read)
  build_routes
  profile_cells "$ROUTE" route ;;
plugin)
  for rep in a b c; do run "plugin-$rep" timeout 55s bun "$HERE/sources/plugin-client-baseline.ts"; done ;;
profile-read)
  build_routes
  run profile-read timeout 55s env EIDNARA_KERNEL_ROUTES_PROFILE=read/1000-rows/narrow-scope perf record -F 99 --call-graph dwarf,8192 -o "$OUT/perf.data" -- "$ROUTE"
  perf report -i "$OUT/perf.data" --no-children --sort comm,dso,sym --stdio -g none > "$OUT/perf-self-by-comm.txt" 2>/dev/null ;;
scout)
  install_scout
  run scout-1000-1 timeout 120s "$SCOUT" 1000 1 21 8
  run scout-1000-64 timeout 120s "$SCOUT" 1000 64 21 8
  run scout-256-1 timeout 120s "$SCOUT" 256 1 21 8
  run scout-8192-1 timeout 900s "$SCOUT" 8192 1 7 8
  run scout-hidden3-1000 timeout 300s env SCOUT_HIDDEN_EVERY=3 "$SCOUT" 1000 1 11 8
  run scout-payload1k-1000 timeout 600s env SCOUT_PAYLOAD_BYTES=1000 "$SCOUT" 1000 1 11 8 ;;
pushdown-join)
  install_scout
  for n in 1000 8192; do run "pushdown-join-$n" timeout 900s env SCOUT_SCAN_DOMAIN_JOIN=1 "$SCOUT" "$n" 1 7 8; done ;;
concurrency)
  install_scout
  run concurrency-1000 timeout 900s env SCOUT_CONCURRENCY=1 "$SCOUT" 1000 1 6 8 ;;
typed-ab)
  test -z "$(git status --porcelain -- crates/daemon)"
  build_routes
  cp "$ROUTE" "$OUT/kernel_routes-baseline"
  git apply "$HERE/evidence/typed-read-body.patch"
  trap 'git checkout -- crates/daemon/src/dispatch.rs crates/daemon/src/kernel_routes/read.rs crates/daemon/benches/kernel_routes.rs; cleanup' EXIT
  build_routes
  cp "$ROUTE" "$OUT/kernel_routes-typed"
  git checkout -- crates/daemon/src/dispatch.rs crates/daemon/src/kernel_routes/read.rs crates/daemon/benches/kernel_routes.rs
  trap cleanup EXIT
  for round in 1 2 3; do
    for variant in baseline typed; do
      for cell in read/1000-rows/narrow-scope read/10-rows/narrow-scope read/1000-rows/wide-scope; do
        run "ab-$variant-${cell//\//-}-r$round" timeout 55s env EIDNARA_KERNEL_ROUTES_PROFILE="$cell" "$OUT/kernel_routes-$variant"
      done
    done
  done
  grep -H profiled "$OUT"/ab-*.log ;;
parity)
  install_scout
  run lower-rust env SCOUT_LOWER=1 "$SCOUT"
  run dump timeout 120s env SCOUT_DUMP="$OUT/route-1000.json" "$SCOUT" 1000 1 2 8
  run rank-ts timeout 55s bun "$HERE/sources/rank-parity.ts" "$OUT/route-1000.json" 8 "remembered fact" "turn 77" "zebra quokka" "SQLite cache ordinary absent another last other word"
  diff <(grep LOWER "$OUT/rank-ts.log" | awk '{print $NF}') <(awk '{print $NF}' "$OUT/lower-rust.log") && echo "lowercase samples identical" ;;
checks)
  run kernel-routes-tests timeout 1500s cargo test --release --locked -p daemon --test kernel_routes
  run dispatch-tests timeout 2400s cargo test --release --locked -p daemon --lib dispatch
  run plugin-check timeout 55s bun test packages/opencode-plugin/src/tools/eidnara-search/kernel-memory-search.test.ts packages/opencode-plugin/src/tools/eidnara-search/render.test.ts ;;
*) echo "Unknown mode: $MODE" >&2; exit 2 ;;
esac
printf 'Evidence: %s\n' "$OUT"
