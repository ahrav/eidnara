/// <reference types="bun-types" />

import { afterEach, describe, expect, it, spyOn } from "bun:test";
import * as logger from "../shared/logger";
import { createMessagesTransformHandler } from "./messages-transform";

type Handler = ReturnType<typeof createMessagesTransformHandler>;
type Output = Parameters<Handler>[1];
type Message = Output["messages"][number];

function makeOutput(): Output {
    return {
        messages: [
            {
                info: { id: "m1", role: "user", sessionID: "ses_test" },
                parts: [{ type: "text", text: "hello" }],
            } as unknown as Message,
        ],
    };
}

const warnSpy = spyOn(console, "warn").mockImplementation(() => {});

afterEach(() => {
    warnSpy.mockClear();
});

describe("createMessagesTransformHandler — ts mode", () => {
    it("returns the same array instance unchanged and does not call the inner hook", async () => {
        let called = false;
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async () => {
                    called = true;
                },
            },
            transformMode: "ts",
        });

        const output = makeOutput();
        const result = await handler({}, output);

        expect(result).toBe(output.messages);
        expect(result).toHaveLength(1);
        expect(called).toBe(false);
    });

    it("warns once per handler across repeated calls", async () => {
        const handler = createMessagesTransformHandler({ eidnara: null, transformMode: "ts" });

        await handler({}, makeOutput());
        await handler({}, makeOutput());
        await handler({}, makeOutput());

        expect(warnSpy).toHaveBeenCalledTimes(1);
        expect(String(warnSpy.mock.calls[0]?.[0])).toContain("transform_mode ts");
    });
});

describe("createMessagesTransformHandler — rust mode", () => {
    it.each([
        "entry",
        "await",
    ])("refuses inherited then at %s without invoking it", async (window) => {
        const key = "then";
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, key);
        let getterCalls = 0;
        let hookCalls = 0;
        const started = Promise.withResolvers<void>();
        const release = Promise.withResolvers<void>();
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async () => {
                    hookCalls += 1;
                    started.resolve();
                    await release.promise;
                },
            },
            transformMode: "rust",
        });
        const output = makeOutput();
        const array = output.messages;
        const member = array[0];
        let result: unknown;
        const pending = window === "await" ? handler({}, output) : undefined;
        if (pending) await started.promise;
        try {
            Object.defineProperty(Array.prototype, key, {
                configurable: true,
                get: () => {
                    getterCalls += 1;
                    return undefined;
                },
            });
            release.resolve();
            result = await (pending ?? handler({}, output));
        } finally {
            if (saved) Object.defineProperty(Array.prototype, key, saved);
            else Reflect.deleteProperty(Array.prototype, key);
        }
        expect(result).toBeUndefined();
        expect(getterCalls).toBe(0);
        expect(hookCalls).toBe(window === "await" ? 1 : 0);
        expect(output.messages).toBe(array);
        expect(array).toHaveLength(1);
        expect(array[0]).toBe(member);
    });

    it("logs a walk-limit decline at warn before the inner hook runs", async () => {
        const warn = spyOn(logger.log, "warn");
        let hookCalls = 0;
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async () => {
                    hookCalls += 1;
                },
            },
            transformMode: "rust",
        });
        // A sparse array exercises the walk-limit path without allocating its elements.
        const output = { messages: new Array(2 ** 22 + 1) as Output["messages"] };
        try {
            expect(await handler({}, output)).toBeUndefined();
            expect(hookCalls).toBe(0);
            expect(
                warn.mock.calls.some((call) => String(call[0]).includes("SourceWalkLimitExceeded")),
            ).toBe(true);
        } finally {
            warn.mockRestore();
        }
    });

    it("checks only the return container after the hook publishes", async () => {
        let getterCalls = 0;
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async (_input, output) => {
                    Object.defineProperty(output.messages[0], "hidden", {
                        get: () => {
                            getterCalls += 1;
                            return "value";
                        },
                    });
                },
            },
            transformMode: "rust",
        });
        const output = makeOutput();
        const array = output.messages;
        expect(await handler({}, output)).toBe(array);
        expect(Object.getOwnPropertyDescriptor(array[0], "hidden")?.get).toBeDefined();
        expect(getterCalls).toBe(0);
    });

    it("calls the inner hook and returns its mutated messages", async () => {
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async (_input, out) => {
                    out.messages.push({
                        info: { id: "injected", role: "user", sessionID: "ses_test" },
                        parts: [{ type: "text", text: "injected" }],
                    } as unknown as Message);
                },
            },
            transformMode: "rust",
        });

        const output = makeOutput();
        const result = await handler({}, output);

        expect(result).toBe(output.messages);
        expect(result).toHaveLength(2);
        expect((result[1]?.info as { id?: string }).id).toBe("injected");
    });

    it("restores the input messages when the inner hook mutates them and then throws", async () => {
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async (_input, out) => {
                    out.messages.splice(0, out.messages.length, {
                        info: { id: "partial", role: "user", sessionID: "ses_test" },
                        parts: [],
                    } as unknown as Message);
                    throw new Error("daemon transform failed after rewriting history");
                },
            },
            transformMode: "rust",
        });

        const output = makeOutput();
        const original = output.messages[0];
        const result = await handler({}, output);

        expect(result).toBe(output.messages);
        expect(result).toHaveLength(1);
        expect(result[0]).toBe(original);
    });

    it("no-ops when eidnara is null", async () => {
        const handler = createMessagesTransformHandler({ eidnara: null, transformMode: "rust" });

        const output = makeOutput();
        const result = await handler({}, output);

        expect(result).toBe(output.messages);
        expect(result).toHaveLength(1);
    });

    it("getEidnara takes precedence over eidnara", async () => {
        let staticCalled = false;
        let dynamicCalled = false;
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async () => {
                    staticCalled = true;
                },
            },
            getEidnara: () => ({
                "experimental.chat.messages.transform": async () => {
                    dynamicCalled = true;
                },
            }),
            transformMode: "rust",
        });

        await handler({}, makeOutput());

        expect(dynamicCalled).toBe(true);
        expect(staticCalled).toBe(false);
    });
});
