import { describe, expect, it, spyOn } from "bun:test";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import * as logger from "../../shared/logger";
import { isNativeCaptureProject, openCodeMemoryCaptureExecutor } from "./memory-capture-native";

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
    directory(): string;
}

function harness(overrides: {
    delete?: (directory: string) => Promise<void>;
    dispose?: (directory: string) => Promise<void>;
    onCreate?: (directory: string) => void;
}): Harness {
    let directory = "";
    const cleanup: string[] = [];
    const client = {
        config: {
            providers: async () => ({
                data: {
                    providers: [
                        {
                            id: "custom",
                            models: { m: { limit: { context: 12000, output: 1000 } } },
                        },
                    ],
                },
            }),
        },
        session: {
            create: async (input: { query: { directory: string }; body: unknown }) => {
                directory = input.query.directory;
                expect(directory).not.toBe(process.cwd());
                expect(isNativeCaptureProject(directory)).toBe(true);
                expect(isNativeCaptureProject(process.cwd())).toBe(false);
                expect(existsSync(join(directory, ".git"))).toBe(true);
                expect(input.body).toMatchObject({
                    title: "eidnara-memory-capture",
                    permission: [{ permission: "*", pattern: "*", action: "deny" }],
                });
                overrides.onCreate?.(directory);
                return { data: { id: "native-child" } };
            },
            prompt: async (input: { query: { directory: string }; body: { model: unknown } }) => {
                expect(input.query.directory).toBe(directory);
                expect(input.body.model).toEqual({ providerID: "custom", modelID: "m" });
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
                expect(input.query.directory).toBe(directory);
                cleanup.push("delete");
                await overrides.delete?.(directory);
            },
        },
        instance: {
            dispose: async (input: { query: { directory: string } }) => {
                expect(input.query.directory).toBe(directory);
                cleanup.push("dispose");
                await overrides.dispose?.(directory);
            },
        },
    };
    return { client, cleanup, directory: () => directory };
}

describe("OpenCode native memory capture executor", () => {
    it("isolates native auth/model execution from repository config and denies tools", async () => {
        const { client, cleanup, directory } = harness({
            onCreate: (directory) => {
                const raw = readFileSync(join(directory, "opencode.json"), "utf8");
                expect(raw).not.toContain("{env:");
                const config = JSON.parse(raw);
                expect(config.agent["eidnara-memory-capture"].permission).toEqual({ "*": "deny" });
                expect(config.agent["eidnara-memory-capture"].tools).toEqual({ "*": false });
                expect(config.provider.custom.models.m.limit.output).toBe(1000);
                expect(config.provider.custom.options).toBeUndefined();
            },
        });
        const executor = openCodeMemoryCaptureExecutor(client as never);
        const result = await executor(work, new AbortController().signal);
        expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
        expect(cleanup).toEqual(["delete", "dispose"]);
        expect(existsSync(directory())).toBe(false);
        expect(isNativeCaptureProject(directory())).toBe(false);
    });

    it("keeps a produced answer when only cleanup fails and releases the admission slot", async () => {
        const warn = spyOn(logger.log, "warn");
        try {
            const directories: string[] = [];
            for (let run = 0; run < 17; run++) {
                const { client, cleanup, directory } = harness({
                    dispose: async () => {
                        throw new Error("dispose refused");
                    },
                });
                const result = await openCodeMemoryCaptureExecutor(client as never)(
                    work,
                    new AbortController().signal,
                );
                expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
                expect(cleanup).toEqual(["delete", "dispose"]);
                expect(existsSync(directory())).toBe(false);
                expect(isNativeCaptureProject(directory())).toBe(false);
                directories.push(directory());
            }
            expect(new Set(directories).size).toBe(17);
            expect(warn).toHaveBeenCalledTimes(17);
            expect(JSON.stringify(warn.mock.calls)).toContain("dispose refused");
        } finally {
            warn.mockRestore();
        }
    });

    it("keeps the recursion guard only while the private directory still exists", async () => {
        // Root ignores directory modes, so an undeletable tree cannot be staged for it.
        if (process.getuid?.() === 0) return;
        let locked = "";
        const { client, directory } = harness({
            dispose: async (directory) => {
                locked = join(directory, "locked");
                mkdirSync(locked);
                writeFileSync(join(locked, "pinned"), "");
                chmodSync(locked, 0o500);
            },
        });
        try {
            const result = await openCodeMemoryCaptureExecutor(client as never)(
                work,
                new AbortController().signal,
            );
            expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
            expect(existsSync(directory())).toBe(true);
            expect(isNativeCaptureProject(directory())).toBe(true);
        } finally {
            if (locked) chmodSync(locked, 0o700);
            rmSync(directory(), { recursive: true, force: true });
        }
        expect(isNativeCaptureProject(directory())).toBe(false);
    });
});
