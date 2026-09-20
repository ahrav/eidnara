import { afterEach, describe, expect, it, spyOn } from "bun:test";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as logger from "../../shared/logger";
import {
    __nativeCaptureTest,
    disposeNativeCaptureProjects,
    isNativeCaptureProject,
    openCodeMemoryCaptureExecutor,
} from "./memory-capture-native";

const work = {
    model: "custom/m",
    system: "Literal {env:PRIVATE_KEY}",
    prompt: "source text",
    maxOutputTokens: 8192,
    maxOutputBytes: 131072,
    maxDurationMs: 90000,
};

interface Harness {
    client: unknown;
    cleanup: string[];
    directories: string[];
    disposed: string[];
    providerReads: number;
}

function harness(overrides: {
    delete?: (directory: string) => Promise<void>;
    dispose?: (directory: string) => Promise<void>;
    /** `instance.dispose` throws synchronously, as a disposed SDK client does. */
    disposeThrowsSync?: boolean;
    onCreate?: (directory: string) => void;
    /** The private session create rejects with this error. */
    createError?: Error;
    /** OpenCode's unified finish reason for the private session's answer. */
    finish?: string;
    /** OpenCode's recorded failure on the private session's answer. */
    error?: unknown;
    /** Settles before the private session's answer is returned. */
    answerGate?: Promise<void>;
    /** The prompt resolves with OpenCode's aborted-message error once the capture signal aborts. */
    abortedAnswer?: boolean;
    /** The private root is created without `git`; the harness skips the `.git` expectation. */
    gitless?: boolean;
}): Harness {
    const directories: string[] = [];
    const disposed: string[] = [];
    const cleanup: string[] = [];
    const client = {
        config: {
            providers: async () => {
                state.providerReads += 1;
                return {
                    data: {
                        providers: [
                            {
                                id: "custom",
                                models: { m: { limit: { context: 12000, output: 1000 } } },
                            },
                        ],
                    },
                };
            },
        },
        session: {
            create: async (input: { query: { directory: string }; body: unknown }) => {
                const directory = input.query.directory;
                directories.push(directory);
                expect(directory).not.toBe(process.cwd());
                expect(isNativeCaptureProject(directory)).toBe(true);
                expect(isNativeCaptureProject(process.cwd())).toBe(false);
                expect(existsSync(join(directory, ".git"))).toBe(overrides.gitless !== true);
                // A `.opencode` directory would make OpenCode install `@opencode-ai/plugin` into it.
                expect(existsSync(join(directory, ".opencode"))).toBe(false);
                if (overrides.createError) throw overrides.createError;
                expect(input.body).toMatchObject({
                    title: "eidnara-memory-capture",
                    permission: [{ permission: "*", pattern: "*", action: "deny" }],
                });
                overrides.onCreate?.(directory);
                return { data: { id: `native-child-${directories.length}` } };
            },
            prompt: async (input: {
                query: { directory: string };
                body: { model: unknown; system?: unknown; parts: unknown };
                signal?: AbortSignal;
            }) => {
                expect(directories).toContain(input.query.directory);
                expect(input.body.model).toEqual({ providerID: "custom", modelID: "m" });
                expect(input.body.system).toBeUndefined();
                expect(input.body.parts).toEqual([{ type: "text", text: work.prompt }]);
                if (overrides.abortedAnswer) {
                    await new Promise<void>((resolve) =>
                        input.signal?.addEventListener("abort", () => resolve(), { once: true }),
                    );
                    return {
                        data: {
                            info: {
                                modelID: "m",
                                providerID: "custom",
                                error: {
                                    name: "MessageAbortedError",
                                    data: { message: "aborted" },
                                },
                            },
                            parts: [],
                        },
                    };
                }
                await overrides.answerGate;
                return {
                    data: {
                        info: {
                            modelID: "m",
                            providerID: "custom",
                            finish: overrides.finish ?? "stop",
                            ...(overrides.error === undefined ? {} : { error: overrides.error }),
                        },
                        parts: [
                            { type: "reasoning", text: "private" },
                            { type: "text", text: '{"ok":true}' },
                        ],
                    },
                };
            },
            abort: async () => {
                cleanup.push("abort");
            },
            delete: async (input: { query: { directory: string } }) => {
                expect(directories).toContain(input.query.directory);
                cleanup.push("delete");
                await overrides.delete?.(input.query.directory);
            },
        },
        instance: {
            dispose: (input: { query: { directory: string } }) => {
                disposed.push(input.query.directory);
                cleanup.push("dispose");
                if (overrides.disposeThrowsSync) throw new Error("client disposed");
                return overrides.dispose?.(input.query.directory) ?? Promise.resolve();
            },
        },
    };
    const state: Harness = { client, cleanup, directories, disposed, providerReads: 0 };
    return state;
}

let lastClient: unknown;

afterEach(async () => {
    if (lastClient) await disposeNativeCaptureProjects(lastClient as never);
    lastClient = undefined;
});

describe("OpenCode native memory capture executor", () => {
    it("isolates native auth/model execution from repository config and denies tools", async () => {
        const configs: string[] = [];
        const h = harness({
            onCreate: (directory) => {
                const raw = readFileSync(join(directory, "opencode.json"), "utf8");
                configs.push(raw);
                expect(raw).not.toContain("{env:");
                const config = JSON.parse(raw);
                expect(config.agent["eidnara-memory-capture"].permission).toEqual({ "*": "deny" });
                expect(config.agent["eidnara-memory-capture"].tools).toEqual({ "*": false });
                expect(config.provider.custom.models.m.limit.output).toBe(1000);
                expect(config.provider.custom.options).toBeUndefined();
            },
        });
        lastClient = h.client;
        const executor = openCodeMemoryCaptureExecutor(h.client as never);
        const result = await executor(work, new AbortController().signal);
        expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
        expect(h.cleanup).toEqual(["delete"]);
        const [directory] = h.directories;
        expect(directory).toBeDefined();
        expect(existsSync(directory as string)).toBe(true);
        expect(isNativeCaptureProject(directory as string)).toBe(true);
    });

    it.each([
        ["stop", "accepted"],
        ["other", "accepted"],
        ["unknown", "accepted"],
        ["length", "output_limit"],
        ["tool-calls", "model_failed"],
        ["content-filter", "model_failed"],
        ["error", "model_failed"],
    ] as const)("maps finish reason %s to %s", async (finish, outcome) => {
        const h = harness({ finish });
        lastClient = h.client;
        const attempt = openCodeMemoryCaptureExecutor(h.client as never)(
            work,
            new AbortController().signal,
        );
        if (outcome === "accepted") {
            expect(await attempt).toEqual({ model: "custom/m", text: '{"ok":true}' });
        } else {
            await expect(attempt).rejects.toThrow(`Native memory capture: ${outcome}`);
        }
    });

    it.each([
        [
            { name: "ProviderAuthError", data: { providerID: "custom", message: "no key" } },
            "provider_unavailable",
        ],
        [
            { name: "APIError", data: { message: "503", statusCode: 503, isRetryable: true } },
            "provider_unavailable",
        ],
        [
            { name: "APIError", data: { message: "reset", isRetryable: true } },
            "provider_unavailable",
        ],
        [
            { name: "APIError", data: { message: "401", statusCode: 401, isRetryable: false } },
            "provider_unavailable",
        ],
        [
            { name: "APIError", data: { message: "400", statusCode: 400, isRetryable: false } },
            "model_failed",
        ],
        [{ name: "ContextOverflowError", data: { message: "too long" } }, "model_failed"],
        [{ name: "UnknownError", data: { message: "boom" } }, "model_failed"],
    ] as const)("maps recorded error %j to %s", async (error, outcome) => {
        const h = harness({ error });
        lastClient = h.client;
        await expect(
            openCodeMemoryCaptureExecutor(h.client as never)(work, new AbortController().signal),
        ).rejects.toThrow(`Native memory capture: ${outcome}`);
    });

    it("evicts a project beyond the bound once its captures finish, not only when the next capture starts", async () => {
        const gate = Promise.withResolvers<void>();
        const h = harness({ answerGate: gate.promise });
        lastClient = h.client;
        const executor = openCodeMemoryCaptureExecutor(h.client as never);
        const signal = new AbortController().signal;
        const busy = ["first", "second", "third", "fourth", "fifth"].map((system) =>
            executor({ ...work, system }, signal),
        );
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
        // Every project is busy, so the admission-time pass finds no victim.
        expect(h.disposed).toEqual([]);
        gate.resolve();
        await Promise.all(busy);
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
        expect(h.disposed).toHaveLength(1);
        expect(new Set(h.directories).size).toBe(5);
    });

    it("keeps a produced answer and settles disposal when instance.dispose throws synchronously", async () => {
        const gate = Promise.withResolvers<void>();
        const h = harness({ disposeThrowsSync: true, answerGate: gate.promise });
        lastClient = h.client;
        const warn = spyOn(logger.log, "warn");
        try {
            const executor = openCodeMemoryCaptureExecutor(h.client as never);
            const busy = executor({ ...work, system: "sync-throw" }, new AbortController().signal);
            await new Promise<void>((resolve) => setTimeout(resolve, 0));
            const retiring = disposeNativeCaptureProjects(h.client as never);
            gate.resolve();
            // The retired project is evicted inside the executor's `finally`; the answer survives.
            await expect(busy).resolves.toEqual({ model: "custom/m", text: '{"ok":true}' });
            await expect(retiring).resolves.toBeUndefined();
            expect(JSON.stringify(warn.mock.calls)).toContain("client disposed");
            for (const directory of h.directories)
                rmSync(directory, { recursive: true, force: true });
        } finally {
            warn.mockRestore();
        }
    });

    it("keeps the directory and recursion guard of a project whose instance cannot be disposed", async () => {
        const h = harness({
            dispose: async () => {
                throw new Error("dispose refused");
            },
        });
        lastClient = h.client;
        const warn = spyOn(logger.log, "warn");
        try {
            await openCodeMemoryCaptureExecutor(h.client as never)(
                work,
                new AbortController().signal,
            );
            const [directory] = h.directories;
            expect(directory).toBeDefined();
            await disposeNativeCaptureProjects(h.client as never);
            expect(h.disposed).toEqual([directory]);
            // A live instance still points at the directory; the guard must outlive the eviction.
            expect(existsSync(directory as string)).toBe(true);
            expect(isNativeCaptureProject(directory as string)).toBe(true);
            expect(JSON.stringify(warn.mock.calls)).toContain("dispose refused");
            rmSync(directory as string, { recursive: true, force: true });
        } finally {
            warn.mockRestore();
        }
    });

    it("reports a failed private session create as unavailable, not as a model failure", async () => {
        const h = harness({ createError: new Error("session service down") });
        lastClient = h.client;
        await expect(
            openCodeMemoryCaptureExecutor(h.client as never)(work, new AbortController().signal),
        ).rejects.toThrow("Native memory capture: provider_unavailable");
    });

    it("reports a capture aborted by its caller as cancelled even when OpenCode records an error", async () => {
        const h = harness({ abortedAnswer: true });
        lastClient = h.client;
        const controller = new AbortController();
        const attempt = openCodeMemoryCaptureExecutor(h.client as never)(work, controller.signal);
        await new Promise<void>((resolve) => setTimeout(resolve, 20));
        controller.abort();
        await expect(attempt).rejects.toThrow("Native memory capture: cancelled");
    });

    it("captures without git when no config sits between the private root and the filesystem root", async () => {
        const emptyPath = mkdtempSync(join(tmpdir(), "eidnara-no-git-path-"));
        const savedPath = process.env.PATH;
        process.env.PATH = emptyPath;
        try {
            const h = harness({ gitless: true, onCreate: () => {} });
            lastClient = h.client;
            const result = await openCodeMemoryCaptureExecutor(h.client as never)(
                { ...work, system: "gitless" },
                new AbortController().signal,
            );
            expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
        } finally {
            process.env.PATH = savedPath;
            rmSync(emptyPath, { recursive: true, force: true });
        }
    });

    it("refuses a gitless private root when a parent directory carries OpenCode config", async () => {
        const base = mkdtempSync(join(tmpdir(), "eidnara-no-git-config-"));
        const emptyPath = join(base, "path");
        const clean = join(base, "clean", "root");
        const shadowed = join(base, "shadowed", "root");
        mkdirSync(emptyPath);
        mkdirSync(clean, { recursive: true });
        mkdirSync(shadowed, { recursive: true });
        writeFileSync(join(base, "shadowed", "opencode.jsonc"), "{}");
        const savedPath = process.env.PATH;
        process.env.PATH = emptyPath;
        try {
            expect(() => __nativeCaptureTest.isolateRoot(shadowed)).toThrow(
                "git is unavailable and",
            );
            // The same walk accepts a root with nothing above it... apart from whatever sits
            // above the test's own temp dir, which the git-backed path never consults.
            let accepted: unknown;
            try {
                __nativeCaptureTest.isolateRoot(clean);
                accepted = true;
            } catch (error) {
                accepted = error;
            }
            expect(accepted === true || String(accepted).includes("git is unavailable and")).toBe(
                true,
            );
            expect(existsSync(join(clean, ".git"))).toBe(false);
        } finally {
            process.env.PATH = savedPath;
            rmSync(base, { recursive: true, force: true });
        }
        // With git present the root becomes a repository and no walk happens.
        const repo = mkdtempSync(join(tmpdir(), "eidnara-git-root-"));
        try {
            __nativeCaptureTest.isolateRoot(repo);
            expect(existsSync(join(repo, ".git"))).toBe(true);
        } finally {
            rmSync(repo, { recursive: true, force: true });
        }
    });

    it("disposes idle projects at once and a busy project after its capture finishes", async () => {
        const gate = Promise.withResolvers<void>();
        const h = harness({ answerGate: gate.promise });
        lastClient = h.client;
        const executor = openCodeMemoryCaptureExecutor(h.client as never);
        const signal = new AbortController().signal;
        const busy = executor(work, signal);
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
        const [directory] = h.directories;
        expect(directory).toBeDefined();
        const disposeAll = disposeNativeCaptureProjects(h.client as never);
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
        // The running capture still owns its directory and session.
        expect(h.disposed).toEqual([]);
        expect(existsSync(directory as string)).toBe(true);
        gate.resolve();
        await busy;
        await disposeAll;
        expect(h.disposed).toEqual([directory]);
        expect(existsSync(directory as string)).toBe(false);
        expect(isNativeCaptureProject(directory as string)).toBe(false);
        // A retired project is never reused.
        await executor(work, signal);
        expect(new Set(h.directories).size).toBe(2);
    });

    it("reuses one warm private project per model and system prompt across captures", async () => {
        const h = harness({});
        lastClient = h.client;
        const executor = openCodeMemoryCaptureExecutor(h.client as never);
        const signal = new AbortController().signal;
        await Promise.all([executor(work, signal), executor(work, signal)]);
        await executor(work, signal);
        expect(new Set(h.directories).size).toBe(1);
        expect(h.providerReads).toBe(1);
        expect(h.cleanup).toEqual(["delete", "delete", "delete"]);
        expect(h.disposed).toEqual([]);
        await executor({ ...work, system: "Other instructions" }, signal);
        expect(new Set(h.directories).size).toBe(2);
        expect(h.providerReads).toBe(2);
    });

    it("keeps a produced answer when only cleanup fails and releases the admission slot", async () => {
        const warn = spyOn(logger.log, "warn");
        try {
            const h = harness({
                delete: async () => {
                    throw new Error("delete refused");
                },
            });
            lastClient = h.client;
            for (let run = 0; run < 17; run++) {
                const result = await openCodeMemoryCaptureExecutor(h.client as never)(
                    work,
                    new AbortController().signal,
                );
                expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
            }
            expect(h.cleanup).toHaveLength(17);
            expect(warn).toHaveBeenCalledTimes(17);
            expect(JSON.stringify(warn.mock.calls)).toContain("delete refused");
        } finally {
            warn.mockRestore();
        }
    });

    it("disposes the least recently used project beyond the bound and keeps the guard of one it cannot remove", async () => {
        // Root ignores directory modes, so an undeletable tree cannot be staged for it.
        if (process.getuid?.() === 0) return;
        const h = harness({});
        lastClient = h.client;
        const executor = openCodeMemoryCaptureExecutor(h.client as never);
        const signal = new AbortController().signal;
        await executor({ ...work, system: "first" }, signal);
        const first = h.directories[0] as string;
        const locked = join(first, "locked");
        mkdirSync(locked);
        writeFileSync(join(locked, "pinned"), "");
        chmodSync(locked, 0o500);
        try {
            for (const system of ["second", "third", "fourth"])
                await executor({ ...work, system }, signal);
            expect(h.disposed).toEqual([]);
            await executor({ ...work, system: "fifth" }, signal);
            await Promise.resolve();
            expect(h.disposed).toEqual([first]);
            expect(existsSync(first)).toBe(true);
            expect(isNativeCaptureProject(first)).toBe(true);
            for (const directory of new Set(h.directories.slice(1)))
                expect(isNativeCaptureProject(directory)).toBe(true);
        } finally {
            chmodSync(locked, 0o700);
            rmSync(first, { recursive: true, force: true });
        }
        expect(isNativeCaptureProject(first)).toBe(false);
    });
});
