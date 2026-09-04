#!/usr/bin/env bash
# Correctness and budget gate for one candidate; exits non-zero at the first failure. Stages
# follow the runbook order. Never edit this file to make an experiment pass; harness changes
# are their own iterations followed by a fresh A/A control.
#
# Env: PROPTEST_CASES (default 50000), GUARD_SKIP_DIFF=1 skips the Bun differential stage
# (Bun or node_modules unavailable), GUARD_COLD_TRADE=1 records an intentional cold-start trade.
set -euo pipefail
cd "$(dirname "$0")/../../.."
BENCH=crates/tokenizer/benches
BASE="$BENCH/baseline.json"
CORE=${BENCH_CORE:-8}

step() { echo "== guard: $*" >&2; }
fail() { echo "GUARD FAIL: $*" >&2; exit 1; }

step "1 workspace gates"
cargo fmt --check
cargo clippy -p tokenizer --all-targets -- -D warnings
RUSTDOCFLAGS=-Dwarnings cargo doc -p tokenizer --no-deps
cargo build -p tokenizer --no-default-features

step "2 crate tests"
out=$(cargo test -p tokenizer --release -q 2>&1) || { echo "$out" | tail -40 >&2; fail "cargo test"; }
echo "$out" | grep -E "test result" >&2

step "3 differential vs ai-tokenizer"
if [[ "${GUARD_SKIP_DIFF:-0}" != "1" ]]; then
    mkdir -p target/diff
    [[ -s target/diff/corpus.jsonl ]] || bun crates/tokenizer/gen/gen-diff-corpus.ts target/diff/corpus.jsonl
    out=$(TOKENIZER_DIFF_CORPUS="$PWD/target/diff/corpus.jsonl" \
        cargo test -p tokenizer --release --lib differential_corpus_matches_ai_tokenizer -- --nocapture 2>&1) ||
        { echo "$out" | grep -E "differ|panicked" | head -30 >&2; fail "differential test"; }
    echo "$out" | grep -E "differential corpus" >&2
else
    echo "differential stage skipped (GUARD_SKIP_DIFF=1)" >&2
fi

step "4+5 metamorphic and reference-parity property tests (${PROPTEST_CASES:-50000} cases)"
out=$(PROPTEST_CASES=${PROPTEST_CASES:-50000} cargo test -p tokenizer --release --lib parity_tests:: -q 2>&1) ||
    { echo "$out" | grep -vE "^\s+(Compiling|Finished|Running)" | tail -40 >&2; fail "parity property tests"; }
echo "$out" | grep -E "test result" >&2

BIN=$(cargo build -p tokenizer --release --bench arms --message-format=json 2>/dev/null |
    python3 -c 'import sys,json
for l in sys.stdin:
    m=json.loads(l)
    if m.get("reason")=="compiler-artifact" and m["target"]["name"]=="arms" and m.get("executable"):
        print(m["executable"])')

step "6 cold-start budget"
cold=$(for _ in $(seq 15); do taskset -c "$CORE" "$BIN" --cold | cut -f2; done | sort -n | sed -n 8p)
echo "cold_start median ns: $cold" >&2

step "7 size budget"
cargo build -p tokenizer --release -q
rlib=$(stat -c %s target/release/libtokenizer.rlib)
binsz=$(stat -c %s "$BIN")
echo "libtokenizer.rlib=$rlib arms=$binsz" >&2

if [[ ! -f "$BASE" ]]; then
    echo "{\"cold_start_ns\": $cold, \"rlib_bytes\": $rlib, \"bin_bytes\": $binsz}" >"$BASE"
    echo "wrote $BASE (first run)" >&2
else
    python3 - "$BASE" "$cold" "$rlib" "$binsz" "${GUARD_COLD_TRADE:-0}" <<'PY'
import json, sys
b = json.load(open(sys.argv[1])); cold, rlib, binsz, trade = map(int, sys.argv[2:6])
def pct(a, c): return (a - c) / c * 100
print(f"cold_start {pct(cold, b['cold_start_ns']):+.1f}%  rlib {pct(rlib, b['rlib_bytes']):+.1f}%  bin {pct(binsz, b['bin_bytes']):+.1f}%", file=sys.stderr)
if cold > b["cold_start_ns"] * 1.20 and not trade:
    sys.exit("GUARD FAIL: cold start over budget (set GUARD_COLD_TRADE=1 for an intentional trade)")
if rlib > b["rlib_bytes"] * 1.15 or binsz > b["bin_bytes"] * 1.15:
    sys.exit("GUARD FAIL: artifact size grew more than 15% over baseline")
PY
fi
echo "GUARD OK"
