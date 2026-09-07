import { describe, expect, it } from "bun:test";
import { copyFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SHARED_DIR = dirname(fileURLToPath(import.meta.url));
const WORKSPACE_ROOT = resolve(SHARED_DIR, "../../../..");
const HEURISTIC_DIVISOR = 3.5;

type RaceReport = {
    loaded: boolean;
    during: number;
    after: number;
    warnings: string[];
};

/**
 * Run with no `node_modules` ancestor so `require("ai-tokenizer")` fails, while `cwd` remains the
 * workspace root for `preloadTokenizer`'s on-disk search.
 */
function runPreloadRace(text: string): RaceReport {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-token-estimator-race-"));
    try {
        copyFileSync(join(SHARED_DIR, "token-estimator.ts"), join(dir, "token-estimator.ts"));
        writeFileSync(
            join(dir, "driver.ts"),
            [
                'import { estimateTokens, preloadTokenizer } from "./token-estimator.ts";',
                "const warnings: string[] = [];",
                'console.warn = (...args: unknown[]) => { warnings.push(args.map(String).join(" ")); };',
                `const text = ${JSON.stringify(text)};`,
                "const preload = preloadTokenizer();",
                "const during = estimateTokens(text);",
                "const loaded = await preload;",
                "const after = estimateTokens(text);",
                "console.log(JSON.stringify({ loaded, during, after, warnings }));",
            ].join("\n"),
        );
        // `--no-install` disables Bun's runtime auto-install, so the bare specifier stays unresolvable.
        const proc = Bun.spawnSync(
            [process.execPath, "--no-install", "run", join(dir, "driver.ts")],
            {
                cwd: WORKSPACE_ROOT,
                env: { ...process.env, XDG_CACHE_HOME: dir },
                stdout: "pipe",
                stderr: "pipe",
            },
        );
        const stdout = proc.stdout.toString();
        const stderr = proc.stderr.toString();
        expect(proc.exitCode, `driver failed:\n${stderr}`).toBe(0);
        const lastLine = stdout.trim().split("\n").at(-1) ?? "";
        return JSON.parse(lastLine) as RaceReport;
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

describe("estimateTokens during an in-flight preloadTokenizer", () => {
    it("defers to the pending preload instead of declaring a permanent fallback", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text);

        expect(report.loaded).toBe(true);
        expect(report.during).toBe(Math.ceil(text.length / HEURISTIC_DIVISOR));
        expect(report.after).not.toBe(report.during);
        expect(report.warnings).toEqual([]);
    });
});
