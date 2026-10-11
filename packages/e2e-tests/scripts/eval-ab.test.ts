import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SCRIPT = join(import.meta.dir, "eval-ab.ts");

function run(root: string, args: string[]) {
    return spawnSync(process.execPath, [SCRIPT, ...args, "--out", join(root, "run")], {
        encoding: "utf8",
        // Without `opencode` on PATH an OpenCode arm fails as its first session opens.
        env: { ...process.env, PATH: "/usr/bin:/bin" },
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
