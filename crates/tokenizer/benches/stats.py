#!/usr/bin/env python3
"""Paired ABBA statistics for report.sh. Reads the TSV report.sh collects
(block, treatment, arm, ns_per_byte), prints the per-arm table, the verdict and,
as the last line, the bare candidate composite. Decision rule is in DESIGN.md."""
import json
import os
import sys

import numpy as np

B = 10_000
RNG = np.random.default_rng(20260904)


def geomean(x):
    return float(np.exp(np.mean(np.log(x))))


def boot_ci(per_block, stat):
    """Percentile bootstrap over blocks of `stat(rows)`; rows is (blocks, arms)."""
    n = per_block.shape[0]
    idx = RNG.integers(0, n, size=(B, n))
    samples = np.array([stat(per_block[i]) for i in idx])
    return float(np.percentile(samples, 2.5)), float(np.percentile(samples, 97.5))


def main():
    path, out_json = sys.argv[1], sys.argv[2]
    rows = [l.rstrip("\n").split("\t") for l in open(path) if l.strip()]
    data = {}  # arm -> block -> treatment -> [values]
    for block, treat, arm, v in rows:
        data.setdefault(arm, {}).setdefault(int(block), {}).setdefault(treat, []).append(float(v))
    cold = data.pop("cold_start", None)
    arms = sorted(data)
    blocks = sorted({b for a in arms for b in data[a]})
    lr = np.zeros((len(blocks), len(arms)))
    cand_med = {}
    for j, a in enumerate(arms):
        cvals = []
        for i, b in enumerate(blocks):
            k = np.mean(data[a][b]["K"])
            c = np.mean(data[a][b]["C"])
            lr[i, j] = np.log(c / k)
            cvals.extend(data[a][b]["C"])
        cand_med[a] = float(np.median(cvals))
    summary = {"arms": {}, "blocks": len(blocks)}
    worst_lo = 0.0
    print(f"{'arm':24s} {'cand ns/B':>10s} {'ratio':>7s} {'95% CI':>17s}")
    for j, a in enumerate(arms):
        point = float(np.exp(np.mean(lr[:, j])))
        lo, hi = boot_ci(lr[:, [j]], lambda r: float(np.exp(np.mean(r))))
        worst_lo = max(worst_lo, lo)
        summary["arms"][a] = {"cand_ns_per_byte": cand_med[a], "ratio": point, "ci": [lo, hi]}
        print(f"{a:24s} {cand_med[a]:10.3f} {point:7.4f} [{lo:7.4f}, {hi:7.4f}]")
    comp_ratio = float(np.exp(np.mean(lr)))
    comp_lo, comp_hi = boot_ci(lr, lambda r: float(np.exp(np.mean(r))))
    composite = geomean([cand_med[a] for a in arms])
    summary.update(composite=composite, composite_ratio=comp_ratio, composite_ci=[comp_lo, comp_hi])
    print(f"{'composite':24s} {composite:10.3f} {comp_ratio:7.4f} [{comp_lo:7.4f}, {comp_hi:7.4f}]")
    if cold:
        k = [v for b in cold.values() for v in b.get("K", [])]
        c = [v for b in cold.values() for v in b.get("C", [])]
        summary["cold_start_ns"] = {"K": float(np.median(k)), "C": float(np.median(c))}
        print(f"{'cold_start (ns, median)':24s} K={np.median(k):.0f} C={np.median(c):.0f} ratio={np.median(c)/np.median(k):.4f}")
    if comp_ratio <= 0.99 and comp_hi < 1.0 and worst_lo <= 1.03:
        verdict = "KEEP"
    elif comp_lo <= 1.0 <= comp_hi and worst_lo <= 1.03:
        verdict = "NEUTRAL"
    else:
        verdict = "DISCARD"
    aa = os.environ.get("REPORT_AA") == "1"
    if aa:
        ok = abs(comp_ratio - 1.0) <= 0.01 and all(
            abs(s["ratio"] - 1.0) <= 0.03 for s in summary["arms"].values()
        )
        verdict = "AA_OK" if ok else "AA_FAIL"
    summary["verdict"] = verdict
    print(f"VERDICT {verdict}")
    with open(out_json, "w") as f:
        json.dump(summary, f, indent=1)
    print(f"{composite:.4f}")


if __name__ == "__main__":
    main()
