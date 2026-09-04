#!/usr/bin/env bash
# Contract in DESIGN.md. Last stdout line is the bare candidate composite (geomean ns/byte over
# arms); the line before it is `VERDICT KEEP|NEUTRAL|DISCARD` (or `AA_OK|AA_FAIL` with --aa).
# Flags: --aa runs the kept binary as its own candidate; --promote copies the current build to
# target/keep/arms after a keep. Env: REPLICATES (6 ABBA blocks), BENCH_CORE (8), ARMS_ITERS (25).
set -euo pipefail
cd "$(dirname "$0")/../../.."

REPLICATES=${REPLICATES:-6}
CORE=${BENCH_CORE:-8}
KEEP=target/keep/arms
OUT=target/report
mkdir -p "$OUT" target/keep

build_arms() {
    cargo build -p tokenizer --release --bench arms --message-format=json 2>/dev/null |
        python3 -c 'import sys,json
for l in sys.stdin:
    m=json.loads(l)
    if m.get("reason")=="compiler-artifact" and m["target"]["name"]=="arms" and m.get("executable"):
        print(m["executable"])'
}

if [[ "${1:-}" == "--promote" ]]; then
    cp "$(build_arms)" "$KEEP"
    echo "promoted $(git rev-parse --short HEAD) -> $KEEP"
    exit 0
fi

CAND=$(build_arms)
[[ -x "$CAND" ]] || { echo "candidate build failed" >&2; exit 2; }
cp "$CAND" "$OUT/cand"
CAND="$OUT/cand"
if [[ ! -x "$KEEP" ]]; then
    echo "no kept binary; promoting candidate as first baseline (A/A)"
    cp "$CAND" "$KEEP"
fi
AA=0
if [[ "${1:-}" == "--aa" ]]; then
    AA=1
    CAND="$KEEP"
fi

TSV="$OUT/raw.tsv"
: >"$TSV"
run() { # treatment binary block
    taskset -c "$CORE" "$2" | awk -v b="$3" -v t="$1" -F'\t' '{print b"\t"t"\t"$1"\t"$2}' >>"$TSV"
    taskset -c "$CORE" "$2" --cold | awk -v b="$3" -v t="$1" -F'\t' '{print b"\t"t"\t"$1"\t"$2}' >>"$TSV"
}
# Blocks alternate KCCK and CKKC so a fixed position effect cannot masquerade as a treatment.
for ((b = 0; b < REPLICATES; b++)); do
    if ((b % 2 == 0)); then
        run K "$KEEP" "$b"; run C "$CAND" "$b"; run C "$CAND" "$b"; run K "$KEEP" "$b"
    else
        run C "$CAND" "$b"; run K "$KEEP" "$b"; run K "$KEEP" "$b"; run C "$CAND" "$b"
    fi
    echo "block $((b + 1))/$REPLICATES done" >&2
done

perf stat -x, -e cycles,instructions,branches,branch-misses,L1-dcache-load-misses,cache-references,cache-misses,l1_dtlb_misses \
    -o "$OUT/perf.csv" -- taskset -c "$CORE" "$CAND" --quick >/dev/null 2>&1 || true

echo "commit $(git rev-parse --short HEAD) vs kept $(sha256sum "$KEEP" | cut -c1-12) core=$CORE blocks=$REPLICATES"
REPORT_AA=$AA python3 crates/tokenizer/benches/stats.py "$TSV" "$OUT/summary.json"
