import { describe, expect, test } from "bun:test";
import { readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const SRC = resolve(import.meta.dir, "..");

function sourceFiles(dir: string, acc: string[] = []): string[] {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) sourceFiles(full, acc);
        else if (/\.tsx?$/.test(entry.name)) acc.push(full);
    }
    return acc;
}

const ALL = sourceFiles(SRC);
const TESTS = ALL.filter((file) => /\.test\.tsx?$/.test(file));
const MODULES = ALL.filter((file) => !/\.test\.tsx?$/.test(file));

/** Not-ported subsystems; a path under any of them reachable from a test is residue. The bundler names inputs relative to `SRC`, so a subsystem directly under `src` has no leading slash and the prefix must also accept the start of the path. commentlint: allow(JUDGE) */
const NOT_PORTED =
    /(^|\/)(memory|dreamer|storage[^/]*|search[^/]*|embedding[^/]*|git-commits|git-anchors|user-memory)(\/|\.ts$)/;

/**
 * Modules no landed test reaches through a runtime import. Type-only modules
 * are erased by the bundler and stay here; the others are waiting for the
 * unit that lands their consumer, named beside each. A module added to the
 * tree without a consumer fails this test; a consumer landing shrinks it.
 */
const AWAITING_CONSUMER = new Map<string, string>([
    ["config/load-outcome.ts", "type-only"],
    ["features/context/sidekick/index.ts", "ctx_memory tool (U3)"],
    ["features/context/smart-notes/compiler-prompt.ts", "smart-note compiler (U3)"],
    ["plugin/rust-tool-backends.ts", "ctx_* tools (U3)"],
    ["plugin/types.ts", "type-only"],
    ["shared/context-limit-provenance.ts", "type-only"],
    ["shared/opencode-config-dir-types.ts", "type-only"],
    ["shared/rpc-types.ts", "type-only"],
    ["shared/format-bytes.ts", "TUI (U4)"],
    ["shared/format-threshold.ts", "TUI (U4)"],
    ["shared/kernel-client-testing/state-table.ts", "tool tests (U3)"],
    ["shared/prompt-surface-a1-golden.ts", "prompt-surface test (U4)"],
    ["shared/subagent-runner.ts", "sidekick (U3)"],
    ["shared/transcript.ts", "hooks (U4)"],
    ["shared/tui-runtime-specifiers.ts", "TUI build (U4)"],
    [
        "testing/module-graph-report.ts",
        "test infrastructure: run as a child process, never imported",
    ],
    ["testing/module-graph.ts", "test infrastructure: imported only by the child process"],
]);

describe("module graph over the landed tree", () => {
    test("the residue pattern matches a not-ported subsystem at the source root and nested under it", () => {
        expect(NOT_PORTED.test("memory/foo.ts")).toBe(true);
        expect(NOT_PORTED.test("dreamer.ts")).toBe(true);
        expect(NOT_PORTED.test("features/memory/foo.ts")).toBe(true);
        expect(NOT_PORTED.test("shared/user-memory.ts")).toBe(true);
        expect(NOT_PORTED.test("shared/memory-guard.ts")).toBe(false);
        expect(NOT_PORTED.test("shared/kernel-client/client.ts")).toBe(false);
    });

    test("no test reaches a not-ported subsystem, and every module without a consumer is named", async () => {
        const report = Bun.spawnSync({
            cmd: ["bun", join(import.meta.dir, "module-graph-report.ts"), ...TESTS],
            cwd: SRC,
            stdout: "pipe",
            stderr: "pipe",
        });
        if (report.exitCode !== 0) {
            throw new Error(`module graph report failed: ${report.stderr.toString()}`);
        }
        const graphs = JSON.parse(report.stdout.toString()) as Record<
            string,
            { inputs: string[]; externals: string[] }
        >;
        const reached = new Set<string>();
        const notPorted: string[] = [];
        for (const graph of Object.values(graphs)) {
            for (const input of graph.inputs) reached.add(resolve(SRC, input));
            for (const path of [...graph.inputs, ...graph.externals]) {
                if (NOT_PORTED.test(path)) notPorted.push(path);
            }
        }
        expect(notPorted).toEqual([]);
        const orphans = MODULES.filter((file) => !reached.has(file))
            .map((file) => relative(SRC, file))
            .sort();
        expect(orphans).toEqual([...AWAITING_CONSUMER.keys()].sort());
    }, 120_000);
});
