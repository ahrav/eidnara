#!/usr/bin/env python3
"""Native materialized claim_validation adapter v2. No attempt replacement."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import statistics
import subprocess
import time

CASES = {"single": (1, 3, 1), "mixed64": (16, 64, 1), "duplicates1024": (16, 1024, 1),
         "distinct256": (256, 256, 1), "remote64": (16, 64, 1), "concurrent64": (16, 64, 4)}
MASK = {24, 25, 26, 27}


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def libraries(binary):
    output = subprocess.check_output(["ldd", str(binary)], text=True)
    paths = []
    for line in output.splitlines():
        for token in line.split():
            if token.startswith("/") and Path(token).is_file():
                paths.append(token)
    return {path: digest(path) for path in paths}


def run(args):
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=False)
    for name in ["home", "tmp"]:
        (root / name).mkdir()
    env = {"PATH": "/usr/bin:/bin", "LANG": "C", "HOME": str(root / "home"),
           "TMPDIR": str(root / "tmp"), "EIDNARA_CLAIM_BENCH_SAMPLES": str(args.samples)}
    binaries = {"A": Path(args.baseline).resolve(), "B": Path(args.candidate).resolve()}
    hashes = {key: digest(path) for key, path in binaries.items()}
    libs = {key: libraries(path) for key, path in binaries.items()}
    templates = ["ABBA", "BAAB"] * (args.blocks // 2)
    assert len(templates) == args.blocks and args.blocks >= 2
    random.Random(args.seed).shuffle(templates)
    assert MASK <= os.sched_getaffinity(0)
    manifest = {"adapter": digest(__file__), "binaries": {k: str(v) for k, v in binaries.items()},
                "hashes": hashes, "libraries": libs, "templates": templates, "seed": args.seed,
                "environment": env, "cwd": str(Path(args.cwd).resolve()), "affinity": sorted(MASK),
                "mode": args.mode, "samples": args.samples}
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2))
    for block, template in enumerate(templates):
        for position, treatment in enumerate(template):
            name = f"block-{block:02}-position-{position+1}-{treatment}"
            path = binaries[treatment]
            assert digest(path) == hashes[treatment]
            assert libraries(path) == libs[treatment]
            argv = [str(path), "--bench"]
            record = {"block": block, "position": position+1, "treatment": treatment,
                      "started_unix_seconds": time.time(), "load_before": os.getloadavg(),
                      "argv": argv, "artifact_sha256": hashes[treatment], "status": "started"}
            started = time.monotonic()
            try:
                p = subprocess.run(argv, cwd=args.cwd, env=env, capture_output=True, text=True,
                                   timeout=120, preexec_fn=lambda: os.sched_setaffinity(0, MASK))
                (root / (name + ".stdout")).write_text(p.stdout)
                (root / (name + ".stderr")).write_text(p.stderr)
                record["returncode"] = p.returncode
                assert p.returncode == 0, p.stderr
                record["load_after"] = os.getloadavg()
                rows = [json.loads(line) for line in p.stdout.splitlines()]
                assert len(rows) == len(CASES) and {r["case"] for r in rows} == set(CASES)
                values = {}
                for row in rows:
                    assert row["schema"] == 2 and row["mode"] == args.mode
                    if args.mode == "timing":
                        assert (row["objects"], row["rows"], row["workers"]) == CASES[row["case"]]
                        assert row["samples_per_worker"] == args.samples
                        assert len(row["results"]) == row["workers"]
                        data = []
                        for affinity, samples in row["results"]:
                            assert affinity == "24-27" and len(samples) == args.samples
                            assert all(isinstance(v, int) and v > 0 for v in samples)
                            data.extend(samples)
                        values[row["case"]] = statistics.mean(data)
                    else:
                        assert row["affinity"] == "24-27" and row["workers"] == 1
                        assert row["outputs_retained_at_close"] is True
                        assert (row["allocation_events"] is None) == row["ledger_overflow"]
                        assert all(isinstance(row[k], int) and row[k] >= 0 for k in ["requested_bytes", "peak_live_bytes"])
                        values[row["case"]] = {k: row[k] for k in ["allocation_events", "requested_bytes", "peak_live_bytes"]}
                assert digest(path) == hashes[treatment]
                assert libraries(path) == libs[treatment]
                record.update(status="ok", values=values)
            except Exception as error:
                if isinstance(error, subprocess.TimeoutExpired):
                    for suffix, data in [("stdout", error.stdout), ("stderr", error.stderr)]:
                        if isinstance(data, bytes):
                            data = data.decode(errors="replace")
                        (root / (name + "." + suffix)).write_text(data or "")
                record.update(status="invalid", error=repr(error))
                raise
            finally:
                record["elapsed_seconds"] = time.monotonic() - started
                (root / (name + ".json")).write_text(json.dumps(record, indent=2))
        print(f"{root.name}: complete block {block+1}/{len(templates)}", flush=True)
    sums = {p.name: digest(p) for p in root.iterdir() if p.is_file()}
    (root / "checksums.json").write_text(json.dumps(sums, indent=2))


def analyze(args):
    from scipy.stats import t
    root = Path(args.output)
    manifest = json.loads((root / "manifest.json").read_text())
    for name, expected in json.loads((root / "checksums.json").read_text()).items():
        assert digest(root / name) == expected
    rows = [json.loads(p.read_text()) for p in sorted(root.glob("block-*.json"))]
    assert len(rows) == len(manifest["templates"]) * 4 and all(r["status"] == "ok" for r in rows)
    report = {}
    for case in CASES:
        if manifest["mode"] == "allocations":
            report[case] = {arm: {metric: sorted(set(r["values"][case][metric] for r in rows if r["treatment"] == arm))
                                  for metric in ["allocation_events", "requested_bytes", "peak_live_bytes"]} for arm in ["A", "B"]}
            continue
        contrasts = []
        for block in range(len(manifest["templates"])):
            selected = [r for r in rows if r["block"] == block]
            assert len(selected) == 4
            contrasts.append(statistics.mean(math.log(r["values"][case]) for r in selected if r["treatment"] == "B")
                             - statistics.mean(math.log(r["values"][case]) for r in selected if r["treatment"] == "A"))
        n = len(contrasts)
        estimate = statistics.mean(contrasts)
        sd = statistics.stdev(contrasts)
        radius = t.ppf(1-0.05/(12*2), n-1) * sd / math.sqrt(n)
        arms = {arm: statistics.mean(r["values"][case] for r in rows if r["treatment"] == arm) for arm in ["A", "B"]}
        report[case] = {"ratio": math.exp(estimate), "interval": [math.exp(estimate-radius), math.exp(estimate+radius)],
                        "block_log_sd": sd, "blocks": n, "process_mean_ns": arms}
    (root / "analysis.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["run", "analyze"])
    parser.add_argument("--output", required=True)
    parser.add_argument("--baseline")
    parser.add_argument("--candidate")
    parser.add_argument("--cwd")
    parser.add_argument("--blocks", type=int, default=12)
    parser.add_argument("--samples", type=int, default=32)
    parser.add_argument("--seed", type=int, default=6272026)
    parser.add_argument("--mode", choices=["timing", "allocations"], default="timing")
    args = parser.parse_args()
    if args.action == "run":
        run(args)
    else:
        analyze(args)
