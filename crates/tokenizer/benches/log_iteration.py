#!/usr/bin/env python3
"""Appends one iteration to results.tsv and .auto/log.jsonl from target/report/summary.json.

usage: log_iteration.py <iteration> <commit> <altitude> <hypothesis> <verdict> <reason> [asi_json]
Reads perf counters from target/report/perf.csv when present."""
import csv
import json
import sys
import time

it, commit, altitude, hyp, verdict, reason = sys.argv[1:7]
asi = json.loads(sys.argv[7]) if len(sys.argv) > 7 else {}
s = json.load(open("target/report/summary.json"))
ev = {}
try:
    for r in csv.reader(open("target/report/perf.csv")):
        if len(r) > 2 and r[0].replace(".", "").isdigit():
            ev[r[2].split(":")[0]] = float(r[0])
except FileNotFoundError:
    pass
ipc = f"{ev['instructions']/ev['cycles']:.3f}" if ev.get("cycles") else ""
ins = f"{ev['instructions']:.4g}" if ev.get("instructions") else ""
per = ";".join(f"{a}={v['ratio']:.4f}[{v['ci'][0]:.4f},{v['ci'][1]:.4f}]" for a, v in s["arms"].items())
cold = f"{s['cold_start_ns']['C']:.0f}" if "cold_start_ns" in s else ""
row = [it, commit, altitude, hyp, f"{s['composite']:.4f}", f"{s['composite_ratio']:.4f}",
       f"{s['composite_ci'][0]:.4f}", f"{s['composite_ci'][1]:.4f}", per, cold, ipc, ins, verdict, reason]
open("crates/tokenizer/benches/results.tsv", "a").write("\t".join(row) + "\n")

metrics = {"composite_ratio": round(s["composite_ratio"], 4), "composite_ci_hi": round(s["composite_ci"][1], 4)}
if cold:
    metrics["cold_start_us"] = round(s["cold_start_ns"]["C"] / 1000, 1)
if ev.get("cycles"):
    metrics["ipc"] = round(ev["instructions"] / ev["cycles"], 3)
    metrics["instructions_g"] = round(ev["instructions"] / 1e9, 3)
    metrics["branch_miss_pct"] = round(ev["branch-misses"] / ev["branches"] * 100, 3)
for a, v in s["arms"].items():
    metrics[f"{a}_ratio"] = round(v["ratio"], 4)
status = {"KEEP": "keep", "keep": "keep", "keep (simpler)": "keep"}.get(verdict, "discard")
if verdict.startswith("discard (guard)"):
    status = "checks_failed"
entry = {"run": int(it), "commit": commit, "metric": round(s["composite"], 4), "metrics": metrics,
         "status": status, "description": f"[{altitude}] {hyp}: {reason}", "timestamp": int(time.time() * 1000),
         "segment": 0, "confidence": None, "asi": {"hypothesis": hyp, "verdict": verdict, **asi}}
open(".auto/log.jsonl", "a").write(json.dumps(entry) + "\n")
print("logged", it, verdict, f"{s['composite']:.3f}")
