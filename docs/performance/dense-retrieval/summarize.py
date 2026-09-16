#!/usr/bin/env python3
"""Summarize retained exploratory samples without significance or tail claims."""

import argparse
import json
from pathlib import Path
import statistics


def stats(values):
    return dict(n=len(values), median=statistics.median(values), mean=statistics.mean(values),
                minimum=min(values), maximum=max(values))


def encode_evidence(value):
    prefix = {key: item for key, item in value.items() if key != "observations"}
    return (json.dumps(prefix, indent=2)[:-2] + ',\n  "observations": [\n' +
            ',\n'.join('    ' + json.dumps(row, separators=(',', ':')) for row in value["observations"]) +
            '\n  ]\n}\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runs", nargs="+", type=Path)
    parser.add_argument("--retain", type=Path)
    parser.add_argument("--reformat", action="store_true", help="format retained JSON with one observation per line")
    args = parser.parse_args()
    for path in args.runs:
        if args.reformat:
            value = json.loads(path.read_text())
            text = encode_evidence(value)
            assert json.loads(text) == value
            path.write_text(text)
            print(path)
            continue
        rows = [json.loads(line) for line in (path / "observations.jsonl").read_text().splitlines()]
        queries = [row for row in rows if row["kind"] == "query"]
        result = {"run": path.name, "metadata": queries[0]["metadata"] if queries else {}, "modes_ms": {}, "phases_ms": {}}
        for mode in sorted({row["mode"] for row in queries}):
            selected = [row for row in queries if row["mode"] == mode and row["observation"] == "warm"]
            result["modes_ms"][mode] = stats([row["wall_ns"] / 1e6 for row in selected])
            result["modes_ms"][mode]["first_ms"] = next(row["wall_ns"] / 1e6 for row in queries if row["mode"] == mode)
            if selected[0].get("work"):
                result["modes_ms"][mode]["work"] = selected[0]["work"]
            if selected[0].get("resources"):
                result["modes_ms"][mode]["resources_mean"] = {
                    key: statistics.mean(row["resources"][key] for row in selected)
                    for key in selected[0]["resources"]}
        phases = [row["phases"] for row in queries if row.get("phases") and row["observation"] == "warm"]
        if phases:
            for key in phases[0]:
                if key.endswith("_ns"):
                    result["phases_ms"][key] = stats([row[key] / 1e6 for row in phases])
        result["builds"] = [row for row in rows if row["kind"] in ["resident_build", "file_build"]]
        result["other"] = [row for row in rows if row["kind"] in ["guard", "concurrent_pair", "projection_pragmas"]]
        if args.retain:
            args.retain.mkdir(parents=True, exist_ok=True)
            destination = args.retain / (path.name + ".json")
            if destination.exists():
                raise FileExistsError(destination)
            manifest = json.loads((path / "manifest.json").read_text())
            manifest["uname"]["node"] = "redacted; shared development host"
            retained = {"manifest": manifest,
                        "metadata": result["metadata"], "observations": [
                            {key: value for key, value in row.items() if key != "metadata"} for row in rows]}
            destination.write_text(encode_evidence(retained))
        result["metadata"] = {key: result["metadata"][key] for key in
                              ["n", "dimension", "k", "page_rows", "selectivity_percent", "sqlite_version", "setup"] if key in result["metadata"]}
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
