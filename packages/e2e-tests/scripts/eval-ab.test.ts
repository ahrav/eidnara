import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { probeCapabilities } from "@eidnara/shm-native";

const SCRIPT = join(import.meta.dir, "eval-ab.ts");
const REPORT = join(import.meta.dir, "eval-ab-report.ts");

function run(root: string, args: string[], extra: { path?: string; cwd?: string } = {}) {
    return spawnSync(process.execPath, [SCRIPT, ...args, "--out", join(root, "run")], {
        encoding: "utf8",
        // Without `opencode` on PATH an OpenCode arm fails as its first session opens.
        env: { ...process.env, PATH: extra.path ?? "/usr/bin:/bin" },
        cwd: extra.cwd,
        timeout: 60_000,
    });
}

describe("eval-ab", () => {
    let root = "";
    const fresh = () => {
        root = mkdtempSync(join(tmpdir(), "ab-eval-exit-"));
        return root;
    };
    const cleanup = () => rmSync(root, { recursive: true, force: true });

    it("exits nonzero when an arm fails", () => {
        const result = run(fresh(), ["--arms", "oc-off", "--sandbox", "off"]);
        try {
            expect(result.stdout).toContain("oc-off: rejected");
            expect(result.status).not.toBe(0);
        } finally {
            cleanup();
        }
    });

    it("refuses a repeated arm before claiming the run directory", () => {
        const result = run(fresh(), ["--arms", "pi-off,pi-off", "--sandbox", "off"]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("pi-off");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });

    it("refuses a flag value outside its documented set", () => {
        for (const flag of [
            ["--enforce-window", "onn"],
            ["--sandbox", "onn"],
        ]) {
            const result = run(fresh(), ["--arms", "pi-off", ...flag]);
            try {
                expect(result.status).not.toBe(0);
                expect(result.stderr).toContain(flag[0] as string);
                expect(existsSync(join(root, "run"))).toBe(false);
            } finally {
                cleanup();
            }
        }
    });

    it("refuses a flag it does not know", () => {
        const result = run(fresh(), [
            "--arms",
            "pi-off",
            "--sandbox",
            "off",
            "--project-sze",
            "large",
        ]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("--project-sze");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });

    it("refuses a numeric flag without a usable number", () => {
        for (const [flag, message] of [
            [["--seed", "typo"], "--seed must be a non-negative number"],
            [["--pace-ms", "-5"], "--pace-ms must be a non-negative number"],
            [["--seed", "--tier"], "needs a value"],
        ] as const) {
            const result = run(fresh(), ["--arms", "pi-off", "--sandbox", "off", ...flag]);
            try {
                expect(result.status).not.toBe(0);
                expect(result.stderr).toContain(message);
                expect(existsSync(join(root, "run"))).toBe(false);
            } finally {
                cleanup();
            }
        }
    });

    it("fails an arm whose work directory could not become a repository", () => {
        fresh();
        // A PATH with `sh` and no `git` makes the repository setup fail.
        const bin = join(root, "bin");
        mkdirSync(bin);
        symlinkSync("/bin/sh", join(bin, "sh"));
        const result = run(root, ["--arms", "oc-off", "--sandbox", "off"], { path: bin });
        try {
            expect(result.status).not.toBe(0);
            expect(result.stdout).toMatch(/oc-off: rejected .*git/);
        } finally {
            cleanup();
        }
    });

    it("resolves a relative fixture path against the invocation directory", () => {
        fresh();
        const result = run(root, ["--arms", "pi-on", "--sandbox", "off", "--fixture-bin", "./fx"], {
            cwd: root,
        });
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain(`${join(root, "fx")} is not an executable file`);
        } finally {
            cleanup();
        }
    });

    it("refuses a stall target the world does not contain", () => {
        const result = run(fresh(), [
            "--arms",
            "pi-on",
            "--sandbox",
            "off",
            "--stall-at",
            "1:9999",
        ]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("1:9999");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });

    it("refuses a stall request when no selected arm runs a daemon", () => {
        const result = run(fresh(), ["--arms", "pi-off", "--sandbox", "off", "--stall-at", "0:1"]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("--stall-at");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });

    it.skipIf(probeCapabilities().available)(
        "refuses an Eidnara arm when the shared-memory addon cannot load",
        () => {
            fresh();
            writeFileSync(join(root, "fx"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
            const result = run(root, [
                "--arms",
                "pi-on",
                "--sandbox",
                "off",
                "--fixture-bin",
                join(root, "fx"),
            ]);
            try {
                expect(result.status).not.toBe(0);
                expect(result.stderr).toContain("shm-native");
                expect(existsSync(join(root, "run"))).toBe(false);
            } finally {
                cleanup();
            }
        },
    );

    it("refuses a missing host fixture before claiming the run directory", () => {
        const result = run(fresh(), [
            "--arms",
            "pi-on",
            "--sandbox",
            "off",
            "--fixture-bin",
            join(root, "no-such-fixture"),
        ]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("direct_host_fixture");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });
});

describe("eval-ab-report", () => {
    it("fails when a requested arm left no results", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-report-"));
        try {
            const arm = join(root, "results/pi-off");
            mkdirSync(arm, { recursive: true });
            writeFileSync(
                join(root, "options.json"),
                JSON.stringify({ arms: ["pi-off", "oc-off"] }),
            );
            writeFileSync(
                join(root, "world.json"),
                JSON.stringify({ sessions: [{ turns: [{}] }] }),
            );
            writeFileSync(
                join(arm, "turns.jsonl"),
                `${JSON.stringify({ arm: "pi-off", kind: "filler", ms: 1 })}\n`,
            );
            writeFileSync(join(arm, "done.json"), "{}");
            const result = spawnSync(process.execPath, [REPORT, root], {
                encoding: "utf8",
                timeout: 60_000,
            });
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("oc-off");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("fails on an arm that recorded every turn but did not finish", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-report-"));
        try {
            const arm = join(root, "results/pi-off");
            mkdirSync(arm, { recursive: true });
            writeFileSync(
                join(root, "world.json"),
                JSON.stringify({ sessions: [{ turns: [{}] }] }),
            );
            writeFileSync(join(root, "options.json"), JSON.stringify({ arms: ["pi-off"] }));
            writeFileSync(
                join(arm, "turns.jsonl"),
                `${JSON.stringify({ arm: "pi-off", kind: "filler", ms: 1 })}\n`,
            );
            const result = spawnSync(process.execPath, [REPORT, root], {
                encoding: "utf8",
                timeout: 60_000,
            });
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("pi-off");
            expect(result.stderr).toContain("did not finish");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("fails on an arm that recorded fewer turns than the world holds", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-report-"));
        try {
            const arm = join(root, "results/pi-off");
            mkdirSync(arm, { recursive: true });
            writeFileSync(
                join(root, "world.json"),
                JSON.stringify({ sessions: [{ turns: [{}, {}, {}] }, { turns: [{}] }] }),
            );
            writeFileSync(join(root, "options.json"), JSON.stringify({ arms: ["pi-off"] }));
            writeFileSync(
                join(arm, "turns.jsonl"),
                `${JSON.stringify({ arm: "pi-off", kind: "filler", ms: 1 })}\n`,
            );
            const result = spawnSync(process.execPath, [REPORT, root], {
                encoding: "utf8",
                timeout: 60_000,
            });
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("pi-off");
            expect(result.stderr).toContain("1/4");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("fails on a malformed record instead of dropping it", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-report-"));
        try {
            const arm = join(root, "results/pi-off");
            mkdirSync(arm, { recursive: true });
            writeFileSync(join(root, "world.json"), JSON.stringify({ sessions: [] }));
            writeFileSync(join(root, "options.json"), JSON.stringify({ arms: ["pi-off"] }));
            writeFileSync(
                join(arm, "turns.jsonl"),
                `${JSON.stringify({ arm: "pi-off", kind: "filler", ms: 1 })}\n{"arm":"pi-off","kind":"pro\n`,
            );
            const result = spawnSync(process.execPath, [REPORT, root], {
                encoding: "utf8",
                timeout: 60_000,
            });
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("turns.jsonl:2");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
