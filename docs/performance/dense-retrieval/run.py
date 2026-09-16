#!/usr/bin/env python3
"""Build and run the research probe in an explicitly selected scratch worktree."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

BASE = "8e0491225a7292ef077c675d44b94f94a24041d3"
HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worktree", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--n", type=int, default=20_000)
    parser.add_argument("--dim", type=int, default=384)
    parser.add_argument("--reps", type=int, default=5)
    parser.add_argument("--k", type=int, default=20)
    parser.add_argument("--page", type=int, default=1024)
    parser.add_argument("--selectivity", type=int, default=100)
    parser.add_argument("--query", type=int, default=1)
    parser.add_argument("--cpu")
    parser.add_argument("--candidates", action="store_true")
    parser.add_argument("--guards", action="store_true")
    parser.add_argument("--kernel-cache-pages", type=int, default=0)
    parser.add_argument("--projection-cache-kib", type=int, default=0)
    parser.add_argument("--file-layer", action="store_true")
    parser.add_argument("--concurrent", action="store_true")
    args = parser.parse_args()
    worktree = args.worktree.resolve()
    if worktree == HERE.parents[2]:
        parser.error("use a detached scratch worktree, not the research checkout")
    if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=worktree, capture_output=True).returncode == 0:
        parser.error("scratch worktree must have detached HEAD")
    root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], cwd=worktree, text=True).strip()).resolve()
    if root != worktree:
        parser.error("--worktree must name the worktree root")
    config_paths = [parent / ".cargo" / name for parent in [worktree, *worktree.parents]
                    for name in ["config", "config.toml"]]
    config_paths += [Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / name
                     for name in ["config", "config.toml"]]
    if any(path.exists() for path in config_paths):
        parser.error("this default-build experiment does not accept ambient Cargo config")
    overrides = [key for key in os.environ if key.startswith(("CARGO_PROFILE_", "CARGO_TARGET_", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC", "CFLAGS", "CPPFLAGS", "LDFLAGS")) or key in ("CC", "CXX", "AR")]
    if overrides:
        parser.error(f"unset build overrides for this default-build experiment: {overrides}")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=worktree, text=True).strip()
    if head != BASE:
        parser.error(f"scratch HEAD must be {BASE}")
    if subprocess.check_output(["git", "diff", "HEAD", "--name-only"], cwd=worktree):
        parser.error("scratch tracked files must be unchanged")
    if args.output.exists():
        parser.error("output already exists; retain previous attempts")
    args.output.mkdir(parents=True)
    for name in ["probe.rs", "candidates.rs", "guards.rs", "file_layer.rs", "run.py"]:
        shutil.copyfile(HERE / name, args.output / name)
    probe = HERE / "probe.rs"
    target = worktree / "crates/retrieval/tests/dense_performance_probe.rs"
    shutil.copyfile(probe, target)
    candidates = HERE / "candidates.rs"
    shutil.copyfile(candidates, target.parent / "support/dense_performance_candidates.rs")
    guards = HERE / "guards.rs"
    shutil.copyfile(guards, target.parent / "support/dense_performance_guards.rs")
    file_layer = HERE / "file_layer.rs"
    shutil.copyfile(file_layer, target.parent / "support/dense_performance_file_layer.rs")
    env = dict(os.environ)
    for key in ["n", "dim", "reps", "k", "page", "selectivity", "query"]:
        env[f"DENSE_{key.upper()}"] = str(getattr(args, key))
    env["DENSE_CANDIDATES"] = str(int(args.candidates))
    env["DENSE_KERNEL_CACHE_PAGES"] = str(args.kernel_cache_pages)
    env["DENSE_PROJECTION_CACHE_KIB"] = str(args.projection_cache_kib)
    env["DENSE_FILE_LAYER"] = str(int(args.file_layer))
    env["DENSE_CONCURRENT"] = str(int(args.concurrent))
    build = ["cargo", "+1.98", "test", "--locked", "-p", "retrieval", "--release",
             "--test", "dense_performance_probe", "--no-run", "--message-format=json"]
    (args.output / "attempt.json").write_text(json.dumps({"build_command":build,"config":vars(args)},default=str,indent=2)+"\n")
    compiled = subprocess.run(build, cwd=worktree, env=env, text=True, capture_output=True)
    (args.output / "build.stdout").write_text(compiled.stdout)
    (args.output / "build.stderr").write_text(compiled.stderr)
    if compiled.returncode:
        print(compiled.stderr, file=sys.stderr)
        for line in compiled.stdout.splitlines():
            item = json.loads(line)
            if item.get("reason") == "compiler-message":
                print(item["message"]["rendered"], file=sys.stderr)
        return compiled.returncode
    artifacts = [json.loads(line) for line in compiled.stdout.splitlines() if line.startswith("{")]
    executable = next(item["executable"] for item in artifacts
                      if item.get("executable") and item.get("target", {}).get("name") == "dense_performance_probe")
    command = [executable, "--ignored", "--exact", "guards::dense_candidate_guards" if args.guards else "dense_performance_probe", "--nocapture"]
    if args.cpu is not None:
        command = ["taskset", "-c", str(args.cpu), *command]
    manifest = {
        "base": head, "probe_sha256": hashlib.sha256(probe.read_bytes()).hexdigest(),
        "candidates_sha256": hashlib.sha256(candidates.read_bytes()).hexdigest(),
        "guards_sha256": hashlib.sha256(guards.read_bytes()).hexdigest(),
        "file_layer_sha256": hashlib.sha256(file_layer.read_bytes()).hexdigest(),
        "binary_sha256": hashlib.sha256(Path(executable).read_bytes()).hexdigest(),
        "command": command, "build_command": build, "uname": platform.uname()._asdict(),
        "rustc": subprocess.check_output(["rustc", "+1.98", "-vV"], text=True),
        "cc": subprocess.check_output(["cc", "--version"], text=True),
        "cpu": json.loads(subprocess.check_output(["lscpu", "-J"], text=True)),
        "load_before": Path("/proc/loadavg").read_text(),
        "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
        "config": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
        "env": {key: value for key, value in env.items()
                if key.startswith("DENSE_") or key in ("RUSTFLAGS", "CARGO_TARGET_DIR")},
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    with (args.output / "run.stdout").open("w") as out, (args.output / "run.stderr").open("w") as err:
        result = subprocess.run(command, cwd=worktree, env=env, stdout=out, stderr=err)
    manifest["exit_code"] = result.returncode
    manifest["load_after"] = Path("/proc/loadavg").read_text()
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    rows = [json.loads(line[line.index('{'):]) for line in (args.output / "run.stdout").read_text().splitlines()
            if '{"kind":"setup"' in line or '"kind":"guard"' in line or line.startswith("{")]
    (args.output / "observations.jsonl").write_text("".join(json.dumps(row) + "\n" for row in rows))
    if result.returncode == 0 and (not rows or not any(row.get("kind") == ("guard" if args.guards else "query") for row in rows)):
        raise RuntimeError("successful process produced no expected observation")
    print(json.dumps({"output": str(args.output), "exit_code": result.returncode, "rows": len(rows)}))
    if result.returncode:
        print((args.output / "run.stderr").read_text(), file=sys.stderr)
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
