import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { probeCapabilities } from "@eidnara/shm-native";
import { pinnedNodeOnPath } from "../src/bedrock-peer/harness-runtime";

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

    // Arm-level behavior needs the pinned Node runtime on PATH, which the Pi arm launches.
    const nodeBin = pinnedNodeOnPath();
    it.skipIf(!nodeBin.ok)("exits nonzero when an arm fails, naming the cause", () => {
        fresh();
        // A PATH with `sh` and the pinned `node`, and no `git`, fails the work directory setup.
        const bin = join(root, "bin");
        mkdirSync(bin);
        symlinkSync("/bin/sh", join(bin, "sh"));
        const result = run(root, ["--arms", "pi-off", "--sandbox", "off"], {
            path: `${bin}:${nodeBin.ok ? nodeBin.root : ""}`,
        });
        try {
            expect(result.status).not.toBe(0);
            expect(result.stdout).toMatch(/pi-off: rejected .*git/);
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
            "--typo",
        ]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("unexpected arguments: --project-sze, --typo");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });

    it("refuses a repeated option and a stray argument", () => {
        for (const [args, message] of [
            [["--project-size", "small", "--project-size", "large"], "--project-size given twice"],
            [["--tier", "xs", "stray"], "unexpected arguments: stray"],
        ] as const) {
            const result = run(fresh(), ["--arms", "pi-off", "--sandbox", "off", ...args]);
            try {
                expect(result.status).not.toBe(0);
                expect(result.stderr).toContain(message);
                expect(existsSync(join(root, "run"))).toBe(false);
            } finally {
                cleanup();
            }
        }
    });

    it("refuses a numeric flag without a usable number", () => {
        for (const [flag, message] of [
            [["--seed", "typo"], "--seed must be a non-negative number"],
            [["--seed", "7.5"], "--seed must be an integer below 2^32"],
            [["--seed", "4294967296"], "--seed must be an integer below 2^32"],
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

    it("refuses an arm whose harness toolchain is not the pinned one", () => {
        // /usr/bin:/bin holds no node, so the Pi arm's pinned Node runtime is absent.
        const result = run(fresh(), ["--arms", "pi-off", "--sandbox", "off"]);
        try {
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("node");
            expect(existsSync(join(root, "run"))).toBe(false);
        } finally {
            cleanup();
        }
    });

    it.skipIf(!probeCapabilities().available || !nodeBin.ok)(
        "refuses an Eidnara arm unless both toolchains are on PATH",
        () => {
            fresh();
            writeFileSync(join(root, "fx"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
            // The pinned Node alone satisfies a Pi arm; an Eidnara arm needs OpenCode and npm too.
            const result = run(
                root,
                ["--arms", "pi-on", "--sandbox", "off", "--fixture-bin", join(root, "fx")],
                { path: `${nodeBin.ok ? nodeBin.root : ""}:/usr/bin:/bin` },
            );
            try {
                expect(result.status).not.toBe(0);
                expect(result.stderr).toContain("Eidnara arms need both toolchains");
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
    it("reports the harness closures' disk beside the other disk columns", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-report-"));
        try {
            const arm = join(root, "results/pi-on");
            mkdirSync(arm, { recursive: true });
            writeFileSync(join(root, "options.json"), JSON.stringify({ arms: ["pi-on"] }));
            writeFileSync(
                join(root, "world.json"),
                JSON.stringify({ sessions: [{ turns: [{}] }] }),
            );
            writeFileSync(
                join(arm, "turns.jsonl"),
                `${JSON.stringify({ arm: "pi-on", kind: "filler", ms: 1 })}\n`,
            );
            writeFileSync(
                join(arm, "sessions.jsonl"),
                `${JSON.stringify({ arm: "pi-on", session: 0, disk: { harness: 1e6, eidnara: 2e6, closures: 300e6 } })}\n`,
            );
            writeFileSync(join(arm, "done.json"), "{}");
            const result = spawnSync(process.execPath, [REPORT, root], {
                encoding: "utf8",
                timeout: 60_000,
            });
            expect(result.status).toBe(0);
            const resources = result.stdout.slice(result.stdout.indexOf("## Resources"));
            expect(resources).toContain("closures disk MB");
            expect(resources).toMatch(/\| pi-on \|.*\| 1 \| 2 \| 300 \|/);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("diagnoses pi-onraw misses beside pi-on misses", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-report-"));
        try {
            const arm = join(root, "results/pi-onraw");
            mkdirSync(arm, { recursive: true });
            writeFileSync(join(root, "options.json"), JSON.stringify({ arms: ["pi-onraw"] }));
            writeFileSync(
                join(root, "world.json"),
                JSON.stringify({ sessions: [{ turns: [{}] }] }),
            );
            writeFileSync(
                join(arm, "turns.jsonl"),
                `${JSON.stringify({ arm: "pi-onraw", session: 0, turn: 0, kind: "probe", factId: "decision:1", factKind: "decision", scope: "in_session", grade: "wrong", ms: 5, error: null })}\n`,
            );
            writeFileSync(join(arm, "done.json"), "{}");
            const result = spawnSync(process.execPath, [REPORT, root], {
                encoding: "utf8",
                timeout: 60_000,
            });
            expect(result.status).toBe(0);
            expect(result.stdout).toContain("- pi-onraw decision:1");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

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
