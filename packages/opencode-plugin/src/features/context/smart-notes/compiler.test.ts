import { afterEach, describe, expect, mock, test } from "bun:test";
import { readFileSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import type { PluginContext } from "../../../plugin/types";
import { _resetKeepSubagentsForTesting, setKeepSubagents } from "../../../shared/keep-subagents";
import { createSmartNoteCapabilities, type SmartNoteCapabilityApi } from "./capabilities";
import {
    compileSmartNoteCheck,
    enforceLiteralCapabilityArguments,
    manifestAdvisoryWarnings,
    normalizeCompiledCheck,
    normalizeCron,
    normalizeManifest,
    parseCompilerOutput,
} from "./compiler";
import { runCompiledSmartNoteCheck } from "./sandbox-runner";

const fakeCap: SmartNoteCapabilityApi = {
    readFile: async (filePath) => (filePath === "ready.txt" ? "ready" : null),
    gitHeadSha: async () => "abc123",
    gitTag: async () => "v1.2.3",
    gitLog: async () => [{ sha: "abc", subject: "initial", authorDate: "2026-01-01T00:00:00Z" }],
    httpGet: async () => ({ status: 200, body: "ok" }),
};

async function withTempDir<T>(fn: (dir: string) => Promise<T>): Promise<T> {
    const dir = await mkdtemp(path.join(tmpdir(), "eidnara-smart-note-compiler-"));
    try {
        return await fn(dir);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
}

const VALID_CHECK = `function check(cap) { return { met: cap.readFile("ready.txt") === "ready" }; }`;

function compilerOutput(compiledCheck: string): string {
    return JSON.stringify({
        compiled_check: compiledCheck,
        manifest: { capabilities: ["readFile"], readFiles: ["ready.txt"] },
        check_cron: "*/15 * * * *",
    });
}

function createCompilerClient(
    outputs: string[],
    options: { prompt?: (input: { signal?: AbortSignal }) => Promise<void> } = {},
) {
    let promptCount = 0;
    const client = {
        session: {
            create: mock(async () => ({ data: { id: "compile-child" } })),
            prompt: mock(async (input: { signal?: AbortSignal }) => {
                promptCount += 1;
                await options.prompt?.(input);
            }),
            messages: mock(async () => ({
                data: [
                    {
                        info: { role: "assistant", time: { created: Date.now() } },
                        parts: [{ type: "text", text: outputs[promptCount - 1] ?? "" }],
                    },
                ],
            })),
            abort: mock(async () => ({ data: true })),
            delete: mock(async () => ({ data: undefined })),
        },
    };
    return client as typeof client & PluginContext["client"];
}

function compileArgs(
    client: PluginContext["client"],
    overrides: Partial<Parameters<typeof compileSmartNoteCheck>[0]> = {},
) {
    return {
        client,
        parentSessionId: "ses-parent",
        sessionDirectory: "/repo/project",
        projectIdentity: "/repo/project",
        note: { id: 7, content: "ship it", surfaceCondition: "when ready.txt says ready" },
        capabilityFactory: () => fakeCap,
        signal: new AbortController().signal,
        deadline: Date.now() + 30_000,
        ...overrides,
    };
}

afterEach(() => {
    _resetKeepSubagentsForTesting();
});

describe("compileSmartNoteCheck", () => {
    test("returns before creating a session when the deadline has already passed", async () => {
        const client = createCompilerClient([compilerOutput(VALID_CHECK)]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, { deadline: Date.now() - 1 }),
        );

        expect(result).toEqual({
            ok: false,
            cancelled: false,
            error: "smart-note compile deadline expired",
        });
        expect(client.session.create).not.toHaveBeenCalled();
        expect(client.session.prompt).not.toHaveBeenCalled();
    });

    test("stops at the deadline instead of giving a fallback model a fresh budget", async () => {
        const hangUntilAborted = (input: { signal?: AbortSignal }) =>
            new Promise<void>((_, reject) => {
                input.signal?.addEventListener("abort", () => reject(new Error("aborted")), {
                    once: true,
                });
            });
        const client = createCompilerClient(
            [compilerOutput(VALID_CHECK), compilerOutput(VALID_CHECK)],
            { prompt: hangUntilAborted },
        );

        const result = await compileSmartNoteCheck(
            compileArgs(client, {
                deadline: Date.now() + 50,
                fallbackModels: ["fallback/model"],
            }),
        );

        expect(result).toEqual({
            ok: false,
            cancelled: false,
            error: "smart-note compile deadline expired",
        });
        expect(client.session.prompt).toHaveBeenCalledTimes(1);
    });

    test("reports an external abort as cancelled", async () => {
        const controller = new AbortController();
        const client = createCompilerClient([compilerOutput(VALID_CHECK)], {
            prompt: async () => {
                controller.abort();
                throw new Error("aborted");
            },
        });

        const result = await compileSmartNoteCheck(
            compileArgs(client, { signal: controller.signal }),
        );

        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.cancelled).toBe(true);
    });

    test("tries a fallback model when the primary output fails normalization", async () => {
        const client = createCompilerClient([
            compilerOutput(`async function check(cap) { return { met: true }; }`),
            compilerOutput(VALID_CHECK),
        ]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, {
                model: "primary/model",
                fallbackModels: ["fallback/model"],
            }),
        );

        expect(result.ok).toBe(true);
        expect(client.session.prompt).toHaveBeenCalledTimes(2);
        const promptCalls = client.session.prompt.mock.calls as unknown as Array<
            [{ body: { model?: { providerID: string; modelID: string } } }]
        >;
        expect(promptCalls[1][0].body.model).toEqual({ providerID: "fallback", modelID: "model" });
    });

    test("tries a fallback model when the primary check fails its dry run", async () => {
        const client = createCompilerClient([
            compilerOutput(`function check(cap) { throw new Error("boom"); }`),
            compilerOutput(VALID_CHECK),
        ]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, { fallbackModels: ["fallback/model"] }),
        );

        expect(result.ok).toBe(true);
        expect(client.session.prompt).toHaveBeenCalledTimes(2);
    });

    test("reports a dry-run failure when no fallback model is configured", async () => {
        const client = createCompilerClient([
            compilerOutput(`function check(cap) { throw new Error("boom"); }`),
        ]);

        const result = await compileSmartNoteCheck(compileArgs(client));

        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.cancelled).toBe(false);
            expect(result.error).toContain("dry-run failed");
        }
        expect(client.session.prompt).toHaveBeenCalledTimes(1);
    });

    test("refuses a computed httpGet URL before the host request runs", async () => {
        const httpGet = mock(async () => ({ status: 200, body: "ok" }));
        const client = createCompilerClient([
            compilerOutput(
                `function check(cap) { cap.httpGet("https://example.com/?d=" + cap.readFile("ready.txt")); return { met: true }; }`,
            ),
        ]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, { capabilityFactory: () => ({ ...fakeCap, httpGet }) }),
        );

        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.error).toContain("not a string literal");
        expect(httpGet).not.toHaveBeenCalled();
    });

    test("deletes the compiler child session unless keep_subagents is set", async () => {
        const deleted = createCompilerClient([compilerOutput(VALID_CHECK)]);
        await compileSmartNoteCheck(compileArgs(deleted));
        expect(deleted.session.delete).toHaveBeenCalledWith({ path: { id: "compile-child" } });

        setKeepSubagents(true);
        const kept = createCompilerClient([compilerOutput(VALID_CHECK)]);
        await compileSmartNoteCheck(compileArgs(kept));
        expect(kept.session.delete).not.toHaveBeenCalled();
    });
});

describe("enforceLiteralCapabilityArguments", () => {
    test("passes literal arguments through and refuses computed ones", async () => {
        const readFile = mock(async (filePath: string) => (filePath === "a.txt" ? "A" : null));
        const httpGet = mock(async () => ({ status: 200, body: "ok" }));
        const code = `function check(cap) {
            cap.readFile("a.txt");
            cap.httpGet('https://example.com/status');
            cap.httpGet(\`https://example.com/tpl\`);
            return { met: true };
        }`;
        const cap = enforceLiteralCapabilityArguments(code, () => ({
            ...fakeCap,
            readFile,
            httpGet,
        }))(new AbortController().signal);

        await expect(cap.readFile("a.txt")).resolves.toBe("A");
        await expect(cap.httpGet("https://example.com/status")).resolves.toEqual({
            status: 200,
            body: "ok",
        });
        await expect(cap.httpGet("https://example.com/tpl")).resolves.toEqual({
            status: 200,
            body: "ok",
        });
        await expect(cap.readFile("b.txt")).rejects.toThrow(/not a string literal/);
        await expect(cap.httpGet("https://example.com/status?x=1")).rejects.toThrow(
            /not a string literal/,
        );
        expect(readFile).toHaveBeenCalledTimes(1);
        expect(httpGet).toHaveBeenCalledTimes(2);
    });
});

describe("smart-note compiler runtime boundary", () => {
    test("blocks .envrc reads at runtime", async () => {
        await withTempDir(async (dir) => {
            await writeFile(path.join(dir, ".envrc"), "SECRET=1", "utf8");
            const result = await runCompiledSmartNoteCheck({
                compiledCheck: `function check(cap) { return { met: cap.readFile(".envrc") !== null }; }`,
                capabilities: createSmartNoteCapabilities({
                    projectRoot: dir,
                    signal: new AbortController().signal,
                }),
            });

            expect(result).toEqual({ ok: true, result: { met: false } });
        });
    });

    test("blocks internal metadata IP fetches at runtime", async () => {
        await withTempDir(async (dir) => {
            const result = await runCompiledSmartNoteCheck({
                compiledCheck: `function check(cap) { cap.httpGet("https://169.254.169.254/latest/meta-data/"); return { met: true }; }`,
                capabilities: createSmartNoteCapabilities({
                    projectRoot: dir,
                    signal: new AbortController().signal,
                }),
            });

            expect(result.ok).toBe(false);
            if (!result.ok) expect(result.error).toContain("internal address");
        });
    });

    test("enforces sandbox time limits", async () => {
        const result = await runCompiledSmartNoteCheck({
            compiledCheck: `function check() { while (true) {} }`,
            capabilities: fakeCap,
            timeoutMs: 100,
        });

        expect(result.ok).toBe(false);
    });

    test("enforces sandbox memory limits", async () => {
        const result = await runCompiledSmartNoteCheck({
            compiledCheck: `function check() { const chunks = []; for (let i = 0; i < 100; i++) chunks.push(new ArrayBuffer(1024 * 1024)); return { met: false }; }`,
            capabilities: fakeCap,
            heapLimitBytes: 64 * 1024,
            timeoutMs: 1_000,
        });

        expect(result.ok).toBe(false);
    });

    test("does not expose the raw host capability bridge to guest code", async () => {
        const result = await runCompiledSmartNoteCheck({
            compiledCheck: `function check(cap) { return { met: cap.readFile("ready.txt") === "ready" && typeof __eidnaraHostCap === "undefined" && !Object.prototype.hasOwnProperty.call(globalThis, "__eidnaraHostCap") }; }`,
            capabilities: fakeCap,
        });

        expect(result).toEqual({ ok: true, result: { met: true } });
    });

    test("treats manifest drift as advisory instead of enforcement", async () => {
        const compiledCheck = `function check(cap) { return { met: cap.readFile("ready.txt") === "ready" }; }`;
        expect(manifestAdvisoryWarnings(compiledCheck, { capabilities: [] })).toContain(
            "manifest omits capability readFile",
        );

        const result = await runCompiledSmartNoteCheck({ compiledCheck, capabilities: fakeCap });
        expect(result).toEqual({ ok: true, result: { met: true } });
    });
});

describe("smart-note compiler output bounds", () => {
    test("bounds compiler output, source, manifest entries, and cron length", () => {
        expect(() => parseCompilerOutput("x".repeat(128 * 1024 + 1))).toThrow(/128 KiB/);
        expect(() =>
            normalizeCompiledCheck(
                `function check() { return { met: false }; }/*${"x".repeat(64 * 1024)}*/`,
            ),
        ).toThrow(/64 KiB/);
        expect(
            normalizeManifest({
                capabilities: [],
                signals: Array.from({ length: 100 }, (_, index) => `signal-${index}`),
            }).signals,
        ).toHaveLength(64);
        expect(() =>
            normalizeManifest({
                capabilities: [],
                signals: Array.from(
                    { length: 4 },
                    (_, index) => `${index}-${"s".repeat(10 * 1024)}`,
                ),
            }),
        ).toThrow(/32 KiB/);
        expect(() => normalizeCron("*".repeat(257))).toThrow(/256 bytes/);
        expect(normalizeCron("  ")).toBe("0 * * * *");
    });

    test("rejects clock reads and Math.random in the compiled check", () => {
        expect(() =>
            normalizeCompiledCheck(`function check() { return { met: Date.now() > 1 }; }`),
        ).toThrow(/clock/);
        expect(() =>
            normalizeCompiledCheck(`function check() { return { met: new Date() > 1 }; }`),
        ).toThrow(/clock/);
        expect(() =>
            normalizeCompiledCheck(`function check() { return { met: Math.random() > 0.5 }; }`),
        ).toThrow(/Math\.random/);
        expect(
            normalizeCompiledCheck(
                `function check() { return { met: new Date("2026-01-01") < new Date("2026-06-01") }; }`,
            ),
        ).toContain("function check()");
    });
});

describe("wire-limit parity with the Rust module (static)", () => {
    const tsSource = readFileSync(path.join(import.meta.dir, "compiler.ts"), "utf8");
    const rustSource = readFileSync(
        path.join(import.meta.dir, "../../../../../../crates/daemon/src/lib.rs"),
        "utf8",
    );

    test("MAX_MANIFEST_BYTES matches NOTE_EVALUATOR_MAX_MANIFEST_BYTES", () => {
        // TypeScript must reject manifests larger than the module limit.
        const tsLimit = tsSource.match(/const MAX_MANIFEST_BYTES = (.+);/)?.[1];
        const rustLimit = rustSource.match(
            /const NOTE_EVALUATOR_MAX_MANIFEST_BYTES: usize = (.+);/,
        )?.[1];
        expect(tsLimit).toBeDefined();
        expect(rustLimit).toBeDefined();
        expect(tsLimit).toBe(rustLimit);
    });

    test("MAX_CRON_BYTES matches NOTE_EVALUATOR_MAX_CRON_BYTES", () => {
        const tsLimit = tsSource.match(/const MAX_CRON_BYTES = (.+);/)?.[1];
        const rustLimit = rustSource.match(
            /const NOTE_EVALUATOR_MAX_CRON_BYTES: usize = (.+);/,
        )?.[1];
        expect(tsLimit).toBeDefined();
        expect(rustLimit).toBeDefined();
        expect(tsLimit).toBe(rustLimit);
    });

    test("MAX_COMPILED_CHECK_BYTES matches NOTE_EVALUATOR_MAX_COMPILED_CHECK_BYTES", () => {
        const tsLimit = tsSource.match(/const MAX_COMPILED_CHECK_BYTES = (.+);/)?.[1];
        const rustLimit = rustSource.match(
            /const NOTE_EVALUATOR_MAX_COMPILED_CHECK_BYTES: usize = (.+);/,
        )?.[1];
        expect(tsLimit).toBeDefined();
        expect(rustLimit).toBeDefined();
        expect(tsLimit).toBe(rustLimit);
    });
});
