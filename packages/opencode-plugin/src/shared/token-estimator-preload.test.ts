import { describe, expect, it } from "bun:test";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SHARED_DIR = dirname(fileURLToPath(import.meta.url));
const WORKSPACE_ROOT = resolve(SHARED_DIR, "../../../..");
const HEURISTIC_DIVISOR = 3.5;
const REAL_TOKENIZER_DIR = dirname(
    createRequire(import.meta.url).resolve("ai-tokenizer/package.json"),
);

type RaceReport = {
    loaded: boolean;
    during: number;
    after: number;
    warnings: string[];
};

type DriverOptions = {
    cwd?: (dir: string) => string;
    /** Plants packages under `dir` before the driver runs. `dir` doubles as `XDG_CACHE_HOME`. */
    prepare?: (dir: string) => void;
};

/**
 * Run with no `node_modules` ancestor so `require("ai-tokenizer")` fails, while `cwd` remains the
 * workspace root for `preloadTokenizer`'s on-disk search.
 */
function runPreloadRace(text: string, options: DriverOptions = {}): RaceReport {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-token-estimator-race-"));
    try {
        // `token-estimator.ts` imports `./error-message`, so the copy needs both modules.
        for (const file of ["token-estimator.ts", "error-message.ts"]) {
            copyFileSync(join(SHARED_DIR, file), join(dir, file));
        }
        options.prepare?.(dir);
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
                cwd: options.cwd ? options.cwd(dir) : WORKSPACE_ROOT,
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

/** An `ai-tokenizer` whose constructor throws a value `String()` cannot convert. */
function plantPoisonTokenizer(packageRoot: string): void {
    mkdirSync(packageRoot, { recursive: true });
    writeFileSync(
        join(packageRoot, "package.json"),
        JSON.stringify({
            name: "ai-tokenizer",
            type: "module",
            exports: { ".": "./index.js", "./encoding/claude": "./claude.js" },
        }),
    );
    writeFileSync(
        join(packageRoot, "index.js"),
        "export default class Tokenizer { constructor() { throw Object.create(null); } }\n",
    );
    writeFileSync(join(packageRoot, "claude.js"), "export const stringEncoder = {};\n");
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

describe("preloadTokenizer installed-package search", () => {
    it("prefers the OpenCode cache install over a package under cwd", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, {
            cwd: (dir) => join(dir, "project"),
            prepare: (dir) => {
                plantPoisonTokenizer(join(dir, "project", "node_modules", "ai-tokenizer"));
                const cacheModules = join(dir, "opencode", "node_modules");
                mkdirSync(cacheModules, { recursive: true });
                symlinkSync(REAL_TOKENIZER_DIR, join(cacheModules, "ai-tokenizer"), "dir");
            },
        });

        expect(report.loaded).toBe(true);
        expect(report.warnings).toEqual([]);
        expect(report.after).not.toBe(Math.ceil(text.length / HEURISTIC_DIVISOR));
    });

    it("reports an unstringifiable loader failure instead of rejecting", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, {
            cwd: (dir) => join(dir, "project"),
            prepare: (dir) => {
                plantPoisonTokenizer(join(dir, "project", "node_modules", "ai-tokenizer"));
            },
        });

        const heuristic = Math.ceil(text.length / HEURISTIC_DIVISOR);
        expect(report.loaded).toBe(false);
        expect(report.during).toBe(heuristic);
        expect(report.after).toBe(heuristic);
        expect(report.warnings).toHaveLength(1);
        expect(report.warnings[0]).toContain("<unstringifiable>");
    });
});
