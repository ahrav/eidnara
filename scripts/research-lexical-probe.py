#!/usr/bin/env python3
"""Run a lexical probe experiment in an isolated source snapshot; retain evidence."""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import tarfile
import time


ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path("crates/retrieval/src/lexical/retrieve.rs")
HARNESS = Path("crates/retrieval/tests/support/lexical_probe_research.rs")
TEST = Path("crates/retrieval/tests/lexical_retrieval.rs")


def sql_constant(source, name):
    marker = f"const {name}: &str ="
    if source.count(marker) != 1:
        raise ValueError(f"ambiguous {name}")
    sql = source.split(marker, 1)[1].strip().split('"', 2)[1]
    if "\\" in sql:
        raise ValueError("escaped Rust SQL is not supported")
    return sql


def retain_report(paths, output, excluded):
    output.mkdir(parents=True, exist_ok=False)
    bundle = []
    report = []
    references = {}
    comparisons = 0
    for path in paths:
        manifest = json.loads((path / "manifest.json").read_text())
        raw = path / "raw.jsonl.gz"
        records = [json.loads(line) for line in gzip.open(raw, "rt")] if raw.exists() else []
        original = subprocess.check_output(
            ["git", "show", f"{manifest['source_head']}:{SOURCE}"], cwd=ROOT, text=True)
        if manifest["variant"] == "baseline":
            expected = hashlib.sha256(sql_constant(original, "PROBE_SQL").encode()).hexdigest()
        else:
            expected = manifest.get("expected_sql_sha256")
        disposition = "complete" if records and records[-1]["kind"] == "complete" else "failed"
        if records and expected is not None and records[0]["production_sql_sha256"] != expected:
            disposition = "invalid_artifact_mismatch"
        # Both arms of an invalid comparison are excluded, including correctly labeled candidate runs.
        if path.name in excluded and disposition == "complete":
            disposition = "excluded_invalid_comparison"
        entry = {"run": path.name, "disposition": disposition, "manifest": manifest, "records": records}
        for name in ["failed.stdout", "failed.stderr", "check.stdout", "check.stderr"]:
            if (path / name).exists():
                entry[name] = (path / name).read_text()
        bundle.append(entry)
        selected = [r for r in summarize(records) if r["key"][0] == "e2e" or
                    (r["key"][0] == "sql" and r["key"][2] == 65)]
        for row in selected:
            row.pop("result", None)
        report.append({"run": path.name, "disposition": disposition, "summary": selected})
        if disposition != "complete":
            continue
        for row in records:
            if row["kind"] != "e2e_sample":
                continue
            key = (row["fixture"], row["total_rows"], row["query"])
            results = [row["result"]] if row["readers"] == 1 else [r["result"] for r in row["requests"]]
            for result in results:
                if key in references:
                    if result != references[key]:
                        raise ValueError(f"retrieval mismatch: {path.name}, {key}")
                    comparisons += 1
                else:
                    references[key] = result
    with gzip.open(output / "evidence.json.gz", "wt", encoding="utf-8") as stream:
        json.dump(bundle, stream, separators=(",", ":"))
    (output / "summary.json").write_text(json.dumps({"exact_result_comparisons": comparisons,
                                                    "runs": report}, indent=2) + "\n")
    hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    (output / "checksums.json").write_text(json.dumps(hashes, indent=2) + "\n")
    print(f"Retained {len(bundle)} runs; {comparisons} exact retrieval comparisons: {output}")


def summarize(records):
    groups = {}
    for row in records:
        if row["kind"] == "sql_sample":
            key = ("sql", row["case"], row["limit"], row["variant"], row["cache_state"])
        elif row["kind"] == "e2e_sample":
            key = ("e2e", row["fixture"], row["query"], row["readers"])
        else:
            continue
        groups.setdefault(key, []).append(row)
    result = []
    for key, rows in groups.items():
        values = [row["elapsed_ns"] / 1e6 for row in rows]
        entry = {"key": key, "n": len(values), "median_ms": statistics.median(values),
                 "mean_ms": statistics.mean(values), "min_ms": min(values), "max_ms": max(values)}
        if key[0] == "sql":
            entry.update(matching_rows=rows[0]["matching_rows"],
                         vm_steps=sorted({r["vm_steps"] for r in rows}),
                         sorts=sorted({r["sorts"] for r in rows}),
                         equal=all(r["ordered_id_rank_equal_to_baseline"] for r in rows))
        else:
            entry["result"] = rows[0]["result"] if rows[0]["readers"] == 1 else rows[0]["requests"][0]["result"]
        result.append(entry)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--retain", type=Path, nargs="+", help="retain existing runs instead of executing")
    parser.add_argument("--exclude-run", action="append", default=[], help="retain but exclude a run from comparisons")
    parser.add_argument("--variant", choices=["baseline", "late_join", "rank_stream"], default="baseline")
    parser.add_argument("--corpus", choices=["diverse", "tied"], default="diverse")
    parser.add_argument("--check", action="store_true", help="also run all non-ignored lexical retrieval tests")
    parser.add_argument("--rows", type=int, default=20000)
    parser.add_argument("--reps", type=int, default=8)
    args = parser.parse_args()
    if args.retain:
        retain_report(args.retain, args.output.resolve(), set(args.exclude_run))
        return
    if args.rows < 2 or args.reps < 2 or args.reps % 2:
        parser.error("rows must be >=2; reps must be positive, even, and >=2")
    output = args.output.resolve()
    if output.is_relative_to(ROOT):
        parser.error("output must be outside the source workspace")
    output.mkdir(parents=True, exist_ok=False)
    scratch = output / "source"
    scratch.mkdir()
    manifest = {"variant": args.variant, "corpus": args.corpus, "rows": args.rows, "reps": args.reps,
                "started_unix": time.time(), "platform": platform.platform(),
                "affinity": sorted(os.sched_getaffinity(0)), "load_before": os.getloadavg(),
                "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
                "commands": [], "status": "started"}
    for name in [SOURCE, HARNESS, TEST, Path(__file__).relative_to(ROOT), Path("Cargo.lock")]:
        manifest.setdefault("input_sha256", {})[str(name)] = hashlib.sha256((ROOT / name).read_bytes()).hexdigest()

    def command(argv, cwd=ROOT, env=None):
        manifest["commands"].append({"argv": [str(a) for a in argv], "cwd": str(cwd)})
        return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, check=True)

    try:
        manifest["source_head"] = command(["git", "rev-parse", "HEAD"]).stdout.decode().strip()
        manifest["rustc"] = command(["rustc", "-vV"]).stdout.decode()
        manifest["cpu"] = command(["lscpu"]).stdout.decode()
        archive = command(["git", "archive", "--format=tar", "HEAD"]).stdout
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(scratch, filter="data")
        # Only the research test files overlay the committed source snapshot.
        for name in [HARNESS, TEST]:
            (scratch / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, scratch / name)
        if args.variant != "baseline":
            source = (scratch / SOURCE).read_text()
            marker = "const PROBE_SQL: &str ="
            if source.count(marker) != 1:
                raise RuntimeError("ambiguous production SQL marker")
            start = source.index('"', source.index(marker))
            end = source.index('";', start + 1) + 1
            harness = (scratch / HARNESS).read_text()
            constant = {"late_join": "LATE_JOIN", "rank_stream": "STREAM_RANK"}[args.variant]
            candidate = '"' + sql_constant(harness, constant) + '"'
            (scratch / SOURCE).write_text(source[:start] + candidate + source[end:])
        manifest["experiment_source_sha256"] = hashlib.sha256((scratch / SOURCE).read_bytes()).hexdigest()
        sql = sql_constant((scratch / SOURCE).read_text(), "PROBE_SQL")
        manifest["expected_sql_sha256"] = hashlib.sha256(sql.encode()).hexdigest()
        env = dict(os.environ)
        # Separate caches prevent a relocated checkout from inheriting another SQL treatment.
        env["CARGO_TARGET_DIR"] = str(ROOT / "target" / "lexical-research" / args.variant)
        build = command(["cargo", "test", "--locked", "--release", "-p", "retrieval",
                         "--test", "lexical_retrieval", "--no-run", "--message-format=json"], scratch, env)
        (output / "build.stderr").write_bytes(build.stderr)
        messages = [json.loads(line) for line in build.stdout.splitlines()]
        executables = [m["executable"] for m in messages if m.get("reason") == "compiler-artifact"
                       and m.get("target", {}).get("name") == "lexical_retrieval" and m.get("executable")]
        if len(executables) != 1:
            raise RuntimeError(f"expected one test executable, found {executables}")
        binary = output / "lexical_retrieval"
        shutil.copy2(executables[0], binary)
        manifest["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        manifest["ldd"] = command(["ldd", str(binary)]).stdout.decode()
        env.update(LEXICAL_RESEARCH_ROWS=str(args.rows), LEXICAL_RESEARCH_REPS=str(args.reps),
                   LEXICAL_RESEARCH_LABEL=output.name, LEXICAL_RESEARCH_VARIANT=args.variant,
                   LEXICAL_RESEARCH_CORPUS=args.corpus,
                   LEXICAL_RESEARCH_EXPECTED_SQL_SHA256=manifest["expected_sql_sha256"])
        manifest["experiment_env"] = {key: value for key, value in env.items()
                                      if key.startswith(("LEXICAL_", "CARGO_PROFILE_", "RUSTFLAGS"))}
        if args.check:
            check = command([str(binary), "--test-threads=1"], scratch, env)
            (output / "check.stdout").write_bytes(check.stdout)
            (output / "check.stderr").write_bytes(check.stderr)
        trial = command([str(binary), "lexical_probe_research::probe_cost_experiment", "--exact",
                         "--ignored", "--nocapture", "--test-threads=1"], scratch, env)
        (output / "run.stderr").write_bytes(trial.stderr)
        records = [json.loads(line.split(b"RESEARCH_JSON ", 1)[1])
                   for line in trial.stdout.splitlines() if b"RESEARCH_JSON " in line]
        with gzip.open(output / "raw.jsonl.gz", "wt", encoding="utf-8") as compressed:
            for record in records:
                compressed.write(json.dumps(record, separators=(",", ":")) + "\n")
        if not records or records[-1]["kind"] != "complete":
            raise RuntimeError("experiment did not complete")
        if records[0]["production_sql_sha256"] != manifest["expected_sql_sha256"]:
            raise RuntimeError("compiled SQL differs from the intended treatment")
        report = summarize(records)
        (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
        (output / "plans.json").write_text(json.dumps([r for r in records if r["kind"] in {"protocol", "sql_plan", "sql_bytecode"}], indent=2) + "\n")
        manifest.update(status="complete", records=len(records), load_after=os.getloadavg())
        for row in report:
            if row["key"][:2] == ("sql", "common") and row["key"][-1] == "warm":
                print(json.dumps(row))
        print(f"Evidence: {output}")
    except subprocess.CalledProcessError as error:
        (output / "failed.stdout").write_bytes(error.stdout)
        (output / "failed.stderr").write_bytes(error.stderr)
        manifest.update(status="failed", returncode=error.returncode)
        raise
    except Exception as error:
        manifest.update(status="failed", error=repr(error))
        raise
    finally:
        manifest["ended_unix"] = time.time()
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        # The binary and input hashes remain; source is reproducible from the pinned commit and overlay.
        shutil.rmtree(scratch)


if __name__ == "__main__":
    main()
