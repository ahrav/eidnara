import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import * as logger from "../../shared/logger";
import {
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
    onCreate?: (directory: string) => void;
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
                expect(existsSync(join(directory, ".git"))).toBe(true);
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
            }) => {
                expect(directories).toContain(input.query.directory);
                expect(input.body.model).toEqual({ providerID: "custom", modelID: "m" });
                expect(input.body.system).toBeUndefined();
                expect(input.body.parts).toEqual([{ type: "text", text: work.prompt }]);
                return {
                    data: {
                        info: { modelID: "m", providerID: "custom", finish: "stop" },
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
            dispose: async (input: { query: { directory: string } }) => {
                disposed.push(input.query.directory);
                cleanup.push("dispose");
                await overrides.dispose?.(input.query.directory);
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
