import { expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { BunPlugin } from "bun";

const TEST_TOKEN_COUNTER_PLUGIN: BunPlugin = {
    name: "test-token-counter",
    setup(build) {
        build.onResolve({ filter: /^@eidnara\/shm-native$/ }, () => ({
            path: join(import.meta.dir, "../../../../test-token-counter.ts"),
        }));
    },
};

it("reuses and invalidates session DB statements on Bun and node:sqlite", async () => {
    const dir = mkdtempSync(join(tmpdir(), "session-db-runtime-"));
    try {
        const built = await Bun.build({
            entrypoints: [join(import.meta.dir, "__tests__/session-db-cache-contract.ts")],
            outdir: dir,
            target: "node",
            format: "esm",
            naming: "[name].mjs",
            plugins: [TEST_TOKEN_COUNTER_PLUGIN],
        });
        expect(built.success, String(built.logs)).toBe(true);
        const results = [];
        for (const runtime of [process.execPath, "node"]) {
            for (const args of [
                [],
                ["normal-chunk"],
                ["growing-chunks"],
                ["finalize-failure", "close"],
                ...[
                    "evicted",
                    "oversized-sql",
                    "oversized-bind",
                    "throwing-sql",
                    "throwing-bind",
                ].flatMap((mode) => [
                    [mode, "close"],
                    [mode, "replace"],
                ]),
            ]) {
                const child = Bun.spawn([runtime, built.outputs[0].path, ...args], {
                    stdout: "pipe",
                    stderr: "pipe",
                });
                const [stdout, stderr, exitCode] = await Promise.all([
                    new Response(child.stdout).text(),
                    new Response(child.stderr).text(),
                    child.exited,
                ]);
                results.push({ runtime, args, stdout, stderr, exitCode });
            }
        }
        expect(results.filter((result) => result.exitCode !== 0)).toEqual([]);
        expect(results[0].stdout).toContain("passed: Bun");
        expect(results.at(-1)?.stdout).toContain("passed: Node.js");
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}, 30_000);
