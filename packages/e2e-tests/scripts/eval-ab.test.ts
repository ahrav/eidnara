import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SCRIPT = join(import.meta.dir, "eval-ab.ts");

describe("eval-ab exit status", () => {
    it("exits nonzero when an arm fails", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-eval-exit-"));
        try {
            // Without `opencode` on PATH the OpenCode arm fails as its first session opens.
            const result = spawnSync(
                process.execPath,
                [SCRIPT, "--arms", "oc-off", "--sandbox", "off", "--out", join(root, "run")],
                {
                    encoding: "utf8",
                    env: { ...process.env, PATH: "/usr/bin:/bin" },
                    timeout: 60_000,
                },
            );
            expect(result.stdout).toContain("oc-off: rejected");
            expect(result.status).not.toBe(0);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
