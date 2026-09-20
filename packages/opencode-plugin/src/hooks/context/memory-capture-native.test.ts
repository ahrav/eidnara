import { expect, it } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { isNativeCaptureProject, openCodeMemoryCaptureExecutor } from "./memory-capture-native";

it("isolates native auth/model execution from repository config and denies tools", async () => {
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
                const raw = readFileSync(join(directory, "opencode.json"), "utf8");
                expect(raw).not.toContain("{env:");
                const config = JSON.parse(raw);
                expect(config.agent["eidnara-memory-capture"].permission).toEqual({ "*": "deny" });
                expect(config.agent["eidnara-memory-capture"].tools).toEqual({ "*": false });
                expect(config.provider.custom.models.m.limit.output).toBe(1000);
                expect(config.provider.custom.options).toBeUndefined();
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
            },
        },
        instance: {
            dispose: async (input: { query: { directory: string } }) => {
                expect(input.query.directory).toBe(directory);
                cleanup.push("dispose");
            },
        },
    };
    const executor = openCodeMemoryCaptureExecutor(client as never);
    const result = await executor(
        {
            model: "custom/m",
            system: "Literal {env:PRIVATE_KEY}",
            prompt: "source text",
            maxOutputTokens: 8192,
            maxOutputBytes: 131072,
            maxDurationMs: 90000,
        },
        new AbortController().signal,
    );
    expect(result).toEqual({ model: "custom/m", text: '{"ok":true}' });
    expect(cleanup).toEqual(["delete", "dispose"]);
    expect(existsSync(directory)).toBe(false);
    expect(isNativeCaptureProject(directory)).toBe(false);
});
