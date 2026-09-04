#!/usr/bin/env bash
# Autoresearch metric wrapper: runs benches/report.sh (paired ABBA vs target/keep/arms) and
# re-emits its summary as METRIC lines. Primary metric is the candidate composite ns/byte.
set -euo pipefail
cd "$(dirname "$0")/.."
R=crates/tokenizer/benches/report.sh
"$R" "$@" >target/report/stdout.txt 2>target/report/stderr.txt || { cat target/report/stderr.txt; exit 1; }
cat target/report/stdout.txt
python3 - <<'PY'
import json, csv
s = json.load(open("target/report/summary.json"))
print(f"METRIC composite_ns_per_byte={s['composite']:.4f}")
print(f"METRIC composite_ratio={s['composite_ratio']:.4f}")
print(f"METRIC composite_ci_hi={s['composite_ci'][1]:.4f}")
for arm, a in s["arms"].items():
    print(f"METRIC {arm}_ratio={a['ratio']:.4f}")
if "cold_start_ns" in s:
    print(f"METRIC cold_start_us={s['cold_start_ns']['C']/1000:.1f}")
try:
    ev = {r[2].split(":")[0]: float(r[0]) for r in csv.reader(open("target/report/perf.csv")) if len(r) > 2 and r[0].replace('.','').isdigit()}
    if ev.get("cycles"):
        print(f"METRIC ipc={ev['instructions']/ev['cycles']:.3f}")
        print(f"METRIC instructions_g={ev['instructions']/1e9:.3f}")
        print(f"METRIC branch_miss_pct={ev['branch-misses']/ev['branches']*100:.3f}")
except FileNotFoundError:
    pass
print(f"VERDICT_LINE {s['verdict']}")
PY
