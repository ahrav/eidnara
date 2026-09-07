import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const SRC = resolve(import.meta.dir);
const PACKAGE_ROOT = resolve(SRC, "..");
const ENTRY = join(SRC, "index.ts");

/** Package boundaries the bundle stops at; the metafile lists nothing behind them. */
const BUILD_EXTERNALS = ["@eidnara/shm-native", "@opencode-ai/plugin", "bun:sqlite", "node:sqlite"];

/** A bundled input under `src/features` or `src/hooks` matching `NOT_PORTED` is residue from a subsystem this package does not contain; the same pattern lives in `testing/module-graph.test.ts`. commentlint: allow(JUDGE) */
const NOT_PORTED = /\/(memory|dreamer|storage[^/]*|search[^/]*|embedding[^/]*)(\/|\.ts$)/;

const EXPECTED_HOOKS = [
    "tool",
    "event",
    "experimental.chat.messages.transform",
    "experimental.chat.system.transform",
    "command.execute.before",
    "chat.message",
    "tool.execute.after",
    "experimental.text.complete",
    "config",
].sort();

const EXPECTED_TOOLS = ["ctx_reduce", "ctx_search", "ctx_note", "ctx_memory"].sort();

interface BuiltEntry {
    outfile: string;
    inputs: string[];
}

/**
 * The bundler runs in a child process: under `bun test`, an in-process
 * `Bun.build` invoked from a test file inside `src/` fails to resolve sibling
 * directories of that file, while the CLI resolves the same graph.
 */
function buildEntry(outdir: string): BuiltEntry {
    const metafilePath = join(outdir, "meta.json");
    const cmd = [
        "bun",
        "build",
        ENTRY,
        "--outdir",
        outdir,
        "--target",
        "node",
        "--format",
        "esm",
        `--metafile=${metafilePath}`,
        ...BUILD_EXTERNALS.flatMap((external) => ["--external", external]),
    ];
    const result = Bun.spawnSync({ cmd, cwd: SRC, stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) {
        throw new Error(`bundle failed: ${result.stderr.toString()}`);
    }
    const metafile = JSON.parse(readFileSync(metafilePath, "utf8")) as {
        inputs?: Record<string, unknown>;
    };
    if (!metafile.inputs) throw new Error("bundle produced no metafile inputs");
    return { outfile: join(outdir, "index.js"), inputs: Object.keys(metafile.inputs) };
}

function fakeClient() {
    return {
        config: {
            get: async () => ({ data: { compaction: { auto: false, prune: false } } }),
            providers: async () => ({
                data: {
                    providers: [
                        {
                            id: "smoke",
                            models: { model: { limit: { context: 200_000, output: 16_000 } } },
                        },
                    ],
                },
            }),
        },
        session: {
            list: async () => [],
            messages: async () => ({ data: [] }),
        },
    };
}

describe("plugin entry bundle", () => {
    let outdir: string;
    let projectDirectory: string;
    let configHome: string;
    let dataHome: string;
    let built: BuiltEntry;
    const savedEnv: Record<string, string | undefined> = {};

    beforeAll(() => {
        // The bundle lives under the package root so its externals resolve through `node_modules`.
        outdir = mkdtempSync(join(PACKAGE_ROOT, ".entry-smoke-dist-"));
        projectDirectory = mkdtempSync(join(tmpdir(), "eidnara-entry-project-"));
        configHome = mkdtempSync(join(tmpdir(), "eidnara-entry-config-"));
        dataHome = mkdtempSync(join(tmpdir(), "eidnara-entry-data-"));
        for (const key of ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "EIDNARA_BROCA_CHILD"]) {
            savedEnv[key] = process.env[key];
        }
        process.env.XDG_CONFIG_HOME = configHome;
        process.env.XDG_DATA_HOME = dataHome;
        delete process.env.EIDNARA_BROCA_CHILD;
        built = buildEntry(outdir);
    }, 60_000);

    afterAll(() => {
        for (const [key, value] of Object.entries(savedEnv)) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
        for (const dir of [outdir, projectDirectory, configHome, dataHome]) {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("no bundled input under features/ or hooks/ belongs to an absent subsystem", () => {
        const residue = built.inputs.filter(
            (input) => /(^|\/)(features|hooks)\//.test(input) && NOT_PORTED.test(input),
        );
        expect(built.inputs.length).toBeGreaterThan(0);
        expect(residue).toEqual([]);
    });

    test("the bundle loads and server() registers the expected hooks and tools", async () => {
        const module = (await import(built.outfile)) as {
            default: {
                id: string;
                server: (ctx: unknown) => Promise<Record<string, unknown>>;
            };
        };
        expect(module.default.id).toBe("eidnara-opencode");

        const hooks = await module.default.server({
            directory: projectDirectory,
            client: fakeClient(),
        });
        try {
            expect(Object.keys(hooks).sort()).toEqual(EXPECTED_HOOKS);
            expect("tool.definition" in hooks).toBe(false);
            expect(Object.keys(hooks.tool as Record<string, unknown>).sort()).toEqual(
                EXPECTED_TOOLS,
            );
        } finally {
            const event = hooks.event as (input: { event: unknown }) => Promise<void>;
            await event({
                event: {
                    type: "server.instance.disposed",
                    properties: { directory: projectDirectory },
                },
            });
        }
    }, 30_000);
});
