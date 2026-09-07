import { describe, expect, it } from "bun:test";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SHARED_DIR = dirname(fileURLToPath(import.meta.url));
const HEURISTIC_DIVISOR = 3.5;
const REAL_TOKENIZER_DIR = dirname(
    createRequire(import.meta.url).resolve("ai-tokenizer/package.json"),
);

type RaceReport = {
    loaded: boolean;
    before?: number;
    during: number;
    after: number;
    warnings: string[];
};

type DriverOptions = {
    /** Plants packages under `dir` before the driver runs. `dir` doubles as `XDG_CACHE_HOME`. */
    prepare?: (dir: string) => void;
    /** `estimateBeforePreload` calls `estimateTokens` before `preloadTokenizer`, forcing a synchronous load. */
    estimateBeforePreload?: boolean;
};

/** The fake OpenCode cache root that `XDG_CACHE_HOME=dir` maps to. */
function cacheNodeModules(dir: string): string {
    return join(dir, "opencode", "node_modules");
}

function linkRealTokenizer(dir: string): void {
    const modules = cacheNodeModules(dir);
    mkdirSync(modules, { recursive: true });
    symlinkSync(REAL_TOKENIZER_DIR, join(modules, "ai-tokenizer"), "dir");
}

/**
 * The driver has no `node_modules` ancestor, so `require("ai-tokenizer")` fails and `preloadTokenizer` searches on disk.
 * `cwd` is an otherwise empty directory under `dir`, so tests can plant a package there and observe whether the search reaches it.
 * commentlint: allow(JUDGE)
 */
function runPreloadRace(text: string, options: DriverOptions = {}): RaceReport {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-token-estimator-race-"));
    try {
        // `token-estimator.ts` imports `./error-message`, which imports `./guarded-read`.
        for (const file of ["token-estimator.ts", "error-message.ts", "guarded-read.ts"]) {
            copyFileSync(join(SHARED_DIR, file), join(dir, file));
        }
        const project = join(dir, "project");
        mkdirSync(project, { recursive: true });
        options.prepare?.(dir);
        writeFileSync(
            join(dir, "driver.ts"),
            [
                'import { estimateTokens, preloadTokenizer } from "./token-estimator.ts";',
                "const warnings: string[] = [];",
                'console.warn = (...args: unknown[]) => { warnings.push(args.map(String).join(" ")); };',
                `const text = ${JSON.stringify(text)};`,
                options.estimateBeforePreload
                    ? "const before = estimateTokens(text);"
                    : "const before = undefined;",
                "const preload = preloadTokenizer();",
                "const during = estimateTokens(text);",
                "const loaded = await preload;",
                "const after = estimateTokens(text);",
                "console.log(JSON.stringify({ loaded, before, during, after, warnings }));",
            ].join("\n"),
        );
        // `--no-install` disables Bun's runtime auto-install, so the bare specifier stays unresolvable.
        const proc = Bun.spawnSync(
            [process.execPath, "--no-install", "run", join(dir, "driver.ts")],
            {
                cwd: project,
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

function plantFakeTokenizer(packageRoot: string, tokenizerSource: string): void {
    mkdirSync(packageRoot, { recursive: true });
    writeFileSync(
        join(packageRoot, "package.json"),
        JSON.stringify({
            name: "ai-tokenizer",
            type: "module",
            exports: { ".": "./index.js", "./encoding/claude": "./claude.js" },
        }),
    );
    writeFileSync(join(packageRoot, "index.js"), tokenizerSource);
    writeFileSync(join(packageRoot, "claude.js"), "export const stringEncoder = {};\n");
}

/** An `ai-tokenizer` whose constructor throws a value `String()` cannot convert. */
const POISON_CONSTRUCTOR =
    "export default class Tokenizer { constructor() { throw Object.create(null); } }\n";

/** An `ai-tokenizer` that constructs but fails on every `encode`. */
const POISON_ENCODE =
    'export default class Tokenizer { encode() { throw new Error("encode boom"); } }\n';

describe("estimateTokens during an in-flight preloadTokenizer", () => {
    it("defers to the pending preload instead of declaring a permanent fallback", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, { prepare: linkRealTokenizer });

        expect(report.loaded).toBe(true);
        expect(report.during).toBe(Math.ceil(text.length / HEURISTIC_DIVISOR));
        expect(report.after).not.toBe(report.during);
        expect(report.warnings).toEqual([]);
    });
});

describe("preloadTokenizer installed-package search", () => {
    it("never consults a package under cwd", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, {
            prepare: (dir) => {
                plantFakeTokenizer(
                    join(dir, "project", "node_modules", "ai-tokenizer"),
                    POISON_CONSTRUCTOR,
                );
            },
        });

        const heuristic = Math.ceil(text.length / HEURISTIC_DIVISOR);
        expect(report.loaded).toBe(false);
        expect(report.after).toBe(heuristic);
        expect(report.warnings).toHaveLength(1);
        expect(report.warnings[0]).toContain("was not found");
    });

    it("reports an unstringifiable loader failure instead of rejecting", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, {
            prepare: (dir) => {
                plantFakeTokenizer(join(cacheNodeModules(dir), "ai-tokenizer"), POISON_CONSTRUCTOR);
            },
        });

        const heuristic = Math.ceil(text.length / HEURISTIC_DIVISOR);
        expect(report.loaded).toBe(false);
        expect(report.during).toBe(heuristic);
        expect(report.after).toBe(heuristic);
        expect(report.warnings).toHaveLength(1);
        expect(report.warnings[0]).toContain("<unstringifiable>");
    });

    it("continues past a broken first candidate to a working later one", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, {
            prepare: (dir) => {
                // The plugin-nested path is searched before the bare cache path.
                plantFakeTokenizer(
                    join(
                        cacheNodeModules(dir),
                        "@eidnara",
                        "opencode",
                        "node_modules",
                        "ai-tokenizer",
                    ),
                    POISON_CONSTRUCTOR,
                );
                linkRealTokenizer(dir);
            },
        });

        expect(report.loaded).toBe(true);
        expect(report.warnings).toEqual([]);
        expect(report.after).not.toBe(Math.ceil(text.length / HEURISTIC_DIVISOR));
    });
});

describe("tokenizer fallback warnings", () => {
    it("warns once for a failed load and once more for a later encode failure", () => {
        const text = "hard bounds ".repeat(40);
        const report = runPreloadRace(text, {
            estimateBeforePreload: true,
            prepare: (dir) => {
                plantFakeTokenizer(join(cacheNodeModules(dir), "ai-tokenizer"), POISON_ENCODE);
            },
        });

        const heuristic = Math.ceil(text.length / HEURISTIC_DIVISOR);
        expect(report.before).toBe(heuristic);
        expect(report.loaded).toBe(true);
        expect(report.after).toBe(heuristic);
        expect(report.warnings).toHaveLength(2);
        expect(report.warnings[0]).toContain("is unavailable");
        expect(report.warnings[1]).toContain("failed to encode");
        expect(report.warnings[1]).toContain("encode boom");
    });
});
