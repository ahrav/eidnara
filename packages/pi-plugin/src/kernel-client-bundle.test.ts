import { describe, expect, it } from "bun:test";
import { readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import {
    bundleModuleGraph,
    databaseBinders,
    type ModuleGraph,
    OPERATION_LITERAL,
    operationLiteralHits,
    reachableModules,
} from "@eidnara/opencode/testing/module-graph";

/** The shared client Pi imports through the `@eidnara/opencode/*` alias. */
const CLIENT_ENTRY = resolve(
    import.meta.dir,
    "../../opencode-plugin/src/shared/kernel-client/index.ts",
);
const PI_ENTRY = resolve(import.meta.dir, "kernel-client-pi.ts");
const CLAIM_STORAGE_PATTERN = /storage-claim/;
/** Reads the harness's own `opencode.db` read-only; the shared plugin names its single writer. */
const HARNESS_DATABASE_READERS = ["hooks/context/read-session-db.ts"].map((reader) =>
    resolve(import.meta.dir, "../../opencode-plugin/src", reader),
);
/** A binding left external (`node:sqlite`, `bun:sqlite`, `better-sqlite3`) or a source module under a `sqlite` path segment. */
const SQLITE_PATTERN =
    /(?:^|\/)(?:node:sqlite|bun:sqlite|better-sqlite3)(?:$|\/)|\/sqlite(?:\.|-|\/)/;

const SRC = resolve(import.meta.dir);
const PACKAGE_ROOT = resolve(SRC, "..");
const MODULE_GRAPH_REPORT = resolve(
    PACKAGE_ROOT,
    "../opencode-plugin/src/testing/module-graph-report.ts",
);

/** The `build` script's real entry points; both bundle roots together must reach every shipped module. */
const BUILD_ENTRIES = ["index.ts", "subagent-entry.ts"].map((entry) => join(SRC, entry));

/** The test runner shares its module registry with the in-process bundler, so build entry graphs run in a child process. */
function buildEntryGraphs(): Record<string, Omit<ModuleGraph, "text">> {
    const report = Bun.spawnSync({
        cmd: ["bun", MODULE_GRAPH_REPORT, ...BUILD_ENTRIES],
        cwd: PACKAGE_ROOT,
        stdout: "pipe",
        stderr: "pipe",
    });
    if (report.exitCode !== 0) {
        throw new Error(`module graph report failed: ${report.stderr.toString()}`);
    }
    return JSON.parse(report.stdout.toString());
}

/**
 * Not-ported subsystems; a path under any of them reachable from a bundle root is residue.
 * `search` is anchored under `features/` so `tools/ctx-search`, a shipped Pi tool, does not match.
 */
const NOT_PORTED =
    /\/(memory|dreamer|storage[^/]*|embedding[^/]*|git-commits|git-anchors|user-memory|context-handler|historian|recomp)(\/|\.ts$)|\/features\/[^/]*\/search[^/]*(\/|\.ts$)/;

/**
 * Runtime-unreachable modules require a documented exclusion here; an entry
 * leaves this map when a runtime import reaches its module.
 */
const AWAITING_CONSUMER = new Map<string, string>([
    [
        "pi-pressure.ts",
        "its consumer wrote session pressure to a session-meta database this package does not port",
    ],
    [
        "read-session-pi.ts",
        "its consumers were the message index and the historian, which read Pi transcripts in TypeScript; the daemon reads transcripts itself",
    ],
]);

function sourceFiles(dir: string, acc: string[] = []): string[] {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) {
            if (entry.name === "__tests__") continue;
            sourceFiles(full, acc);
        } else if (/\.ts$/.test(entry.name) && !/\.(test|typecheck)\.ts$/.test(entry.name)) {
            acc.push(full);
        }
    }
    return acc;
}

describe("Pi kernel-client bundle reachability", () => {
    it("reaches no SQLite binding, claim storage, or claim.*/dreamer.* literal from the kernel-client entry", async () => {
        const graph = await bundleModuleGraph(CLIENT_ENTRY);
        expect(graph.inputs.length).toBeGreaterThan(0);
        expect(reachableModules(graph, SQLITE_PATTERN)).toEqual([]);
        expect(reachableModules(graph, CLAIM_STORAGE_PATTERN)).toEqual([]);
        expect(graph.text).not.toMatch(OPERATION_LITERAL);
    });

    it("reaches no claim storage or claim.*/dreamer.* literal from Pi's resolver and leaves the native host module external", async () => {
        const graph = await bundleModuleGraph(PI_ENTRY);
        expect(graph.inputs.length).toBeGreaterThan(0);
        expect(reachableModules(graph, CLAIM_STORAGE_PATTERN)).toEqual([]);
        expect(graph.text).toMatch(/from\s+["']@eidnara\/shm-native["']/);
        expect(graph.text).not.toMatch(OPERATION_LITERAL);
    });

    it("the shipped entry points reach only the read-only harness-database reader and carry no claim.*/dreamer.* literal", () => {
        const sources = new Set<string>();
        for (const graph of Object.values(buildEntryGraphs())) {
            for (const binder of databaseBinders(graph)) {
                expect(HARNESS_DATABASE_READERS).toContain(resolve(PACKAGE_ROOT, binder));
            }
            for (const input of graph.inputs) {
                if (!input.includes("/node_modules/")) sources.add(resolve(PACKAGE_ROOT, input));
            }
        }
        expect(sources.size).toBeGreaterThan(0);
        expect(operationLiteralHits([...sources])).toEqual([]);
    }, 120_000);

    it("the shipped entry points bundle under the build script's externals", () => {
        const graphs = buildEntryGraphs();
        expect(Object.keys(graphs).sort()).toEqual([...BUILD_ENTRIES].sort());
        for (const graph of Object.values(graphs)) {
            expect(graph.inputs.length).toBeGreaterThan(0);
        }
    }, 120_000);

    it("no bundle root reaches a not-ported subsystem, and every module without a consumer is named", () => {
        const reached = new Set<string>();
        const notPorted: string[] = [];
        for (const graph of Object.values(buildEntryGraphs())) {
            for (const input of graph.inputs) reached.add(resolve(PACKAGE_ROOT, input));
            for (const path of [...graph.inputs, ...graph.externals]) {
                // Third-party packages (typebox ships a `system/memory/` tree) are outside the not-ported scan.
                if (path.includes("/node_modules/")) continue;
                if (NOT_PORTED.test(path)) notPorted.push(path);
            }
        }
        expect(notPorted).toEqual([]);
        const orphans = sourceFiles(SRC)
            .filter((file) => !reached.has(file))
            .map((file) => relative(SRC, file))
            .sort();
        expect(orphans).toEqual([...AWAITING_CONSUMER.keys()].sort());
    }, 120_000);
});
