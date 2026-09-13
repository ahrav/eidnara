import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { spawnSync } from "node:child_process";

const SCRIPT = join(import.meta.dir, "canonical-output-paired-runs.sh");

let repo: string;
let shims: string;
let log: string;

function git(...args: string[]): string {
    const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
    if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
    return result.stdout.trim();
}

function shim(name: string, body: string): void {
    const path = join(shims, name);
    writeFileSync(path, `#!/usr/bin/env bash\n${body}`);
    chmodSync(path, 0o755);
}

// `failBuilds` is a comma-separated list of build ordinals the `cargo` shim rejects.
function installShims(failBuilds: string): void {
    shim(
        "cargo",
        `
echo "cargo $*" >>"${log}"
shift
case "$1" in
  --version) echo "cargo 0.0.0 (shim)";;
  tree) echo "daemon v0.1.0 bench-internals,test-support";;
  build)
    builds=$(grep -c '^cargo +[^ ]* build ' "${log}")
    case ",${failBuilds}," in *",$builds,"*) echo "shim build $builds failed" >&2; exit 101;; esac
    target=""
    while [ $# -gt 0 ]; do
      if [ "$1" = --target-dir ]; then target="$2"; fi
      shift
    done
    mkdir -p "$target/release/examples"
    driver="$target/release/examples/canonical_output_evidence"
    cat >"$driver" <<'DRIVER'
#!/usr/bin/env bash
out=""
while [ $# -gt 0 ]; do
  if [ "$1" = --out ]; then out="$2"; fi
  shift
done
echo "driver $out" >>"$DRIVER_LOG"
printf '{"kind":"canonical-output-evidence/v2","provenance":{"cpu_model":"shim cpu"}}' >"$out"
DRIVER
    chmod +x "$driver"
    ;;
  *) echo "unexpected cargo subcommand $1" >&2; exit 2;;
esac
`,
    );
    shim("rustc", `echo "rustc 0.0.0 (shim)"`);
}

function run(...args: string[]): { status: number | null; stderr: string; stdout: string } {
    return runIn(repo, ...args);
}

function runIn(cwd: string, ...args: string[]): { status: number | null; stderr: string; stdout: string } {
    const result = spawnSync("bash", [SCRIPT, ...args], {
        cwd,
        encoding: "utf8",
        env: {
            ...process.env,
            PATH: `${shims}${delimiter}${process.env.PATH ?? ""}`,
            DRIVER_LOG: log,
            EIDNARA_MICRO_SAMPLES: "1",
            EIDNARA_TRANSFORM_SAMPLES: "1",
        },
    });
    return { status: result.status, stderr: result.stderr, stdout: result.stdout };
}

function logLines(): string[] {
    return existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").filter(Boolean) : [];
}

beforeEach(() => {
    repo = mkdtempSync(join(tmpdir(), "canonical-output-paired-runs-"));
    shims = join(repo, "shims");
    mkdirSync(shims);
    log = join(repo, "invocations.log");
    git("init", "-q");
    git("config", "user.email", "test@example.com");
    git("config", "user.name", "test");
    writeFileSync(join(repo, "Cargo.toml"), "[workspace]\n");
    git("add", "Cargo.toml");
    git("commit", "-q", "-m", "baseline");
    writeFileSync(join(repo, "Cargo.toml"), "[workspace]\n# candidate\n");
    git("commit", "-q", "-am", "candidate");
});

afterEach(() => {
    rmSync(repo, { recursive: true, force: true });
});

describe("canonical-output-paired-runs.sh", () => {
    test("a failed baseline build stops the paired run before the candidate build", () => {
        installShims("1");
        const { status } = run("paired", "HEAD~1", "HEAD", join(repo, "out"));
        expect(status).not.toBe(0);
        const builds = logLines().filter((line) => / build /.test(line));
        expect(builds).toHaveLength(1);
        expect(logLines().some((line) => line.startsWith("driver "))).toBe(false);
        expect(existsSync(join(repo, "out", "provenance.json"))).toBe(false);
    });

    test("baseline mode runs ten processes and writes provenance", () => {
        installShims("");
        const out = join(repo, "out");
        const { status, stderr } = run("baseline", "HEAD~1", out);
        expect(status).toBe(0);
        if (stderr.includes("error")) throw new Error(stderr);
        const drivers = logLines().filter((line) => line.startsWith("driver "));
        expect(drivers).toHaveLength(10);
        expect(drivers[0]).toBe(`driver ${join(out, "run-01.json")}`);
        expect(drivers[9]).toBe(`driver ${join(out, "run-10.json")}`);
        const provenance = JSON.parse(readFileSync(join(out, "provenance.json"), "utf8"));
        expect(provenance.kind).toBe("canonical-output-paired-runs/v1");
        expect(provenance.mode).toBe("baseline");
        expect(provenance.baseline_commit).toBe(git("rev-parse", "HEAD~1"));
        expect(provenance.schedule).toEqual(
            Array.from({ length: 10 }, (_, i) => `run-${String(i + 1).padStart(2, "0")}:A`),
        );
        expect(provenance.resolved_features_baseline).toEqual(["daemon v0.1.0 bench-internals,test-support"]);
        // CPU identity lives in each run file's `provenance.cpu_model`, written by the driver.
        expect(provenance).not.toHaveProperty("cpu_model");
    });

    test("paired mode alternates AB and BA and builds each revision once", () => {
        installShims("");
        const out = join(repo, "out");
        const { status } = run("paired", "HEAD~1", "HEAD", out);
        expect(status).toBe(0);
        const builds = logLines().filter((line) => / build /.test(line));
        expect(builds).toHaveLength(2);
        const drivers = logLines()
            .filter((line) => line.startsWith("driver "))
            .map((line) => line.replace(`driver ${out}/`, ""));
        expect(drivers.slice(0, 4)).toEqual(["pair-01-A.json", "pair-01-B.json", "pair-02-B.json", "pair-02-A.json"]);
        expect(drivers).toHaveLength(20);
        const provenance = JSON.parse(readFileSync(join(out, "provenance.json"), "utf8"));
        expect(provenance.schedule.slice(0, 2)).toEqual(["pair-01:AB", "pair-02:BA"]);
        expect(provenance.candidate_commit).toBe(git("rev-parse", "HEAD"));
    });

    test("a relative output directory resolves against the caller's directory, not the repo root", () => {
        installShims("");
        const sub = join(repo, "sub");
        mkdirSync(sub);
        const { status } = runIn(sub, "baseline", "HEAD~1", "out");
        expect(status).toBe(0);
        expect(existsSync(join(sub, "out", "provenance.json"))).toBe(true);
        expect(existsSync(join(repo, "out"))).toBe(false);
    });
});
