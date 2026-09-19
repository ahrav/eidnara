/// <reference types="bun-types" />

import { describe, expect, it, spyOn } from "bun:test";
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

describe("createMessagesTransformHandler", () => {
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

    for (const window of ["entry", "await"] as const) {
        it.each([
            "own-then",
            "root-proxy",
            "prototype-proxy",
        ])(`logs and refuses %s at ${window} without invoking traps`, async (unsupported) => {
            const key = "then";
            const logSpy = spyOn(logger.log, "debug");
            let trapCalls = 0;
            let hookCalls = 0;
            const trap = () => {
                trapCalls += 1;
                throw new Error("wrapper invoked a source trap");
            };
            const handlers = {
                get: trap,
                has: trap,
                getPrototypeOf: trap,
                getOwnPropertyDescriptor: trap,
                ownKeys: trap,
            };
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
            });
            const output = makeOutput();
            const array = output.messages;
            const member = array[0];
            const pending = window === "await" ? handler({}, output) : undefined;
            try {
                if (pending) await started.promise;
                if (unsupported === "own-then") Object.defineProperty(array, key, { get: trap });
                else if (unsupported === "root-proxy") output.messages = new Proxy(array, handlers);
                else Object.setPrototypeOf(array, new Proxy(Array.prototype, handlers));
                const current = output.messages;
                release.resolve();
                expect(await (pending ?? handler({}, output))).toBeUndefined();
                expect(trapCalls).toBe(0);
                expect(hookCalls).toBe(window === "await" ? 1 : 0);
                expect(output.messages).toBe(current);
                expect(array[0]).toBe(member);
                const rejection =
                    unsupported === "own-then"
                        ? "extra_property at /then"
                        : unsupported === "root-proxy"
                          ? "proxy at /"
                          : "prototype at /";
                expect(logSpy).toHaveBeenCalledWith(
                    `[eidnara] transform declined: ${rejection} (${window === "entry" ? "entry" : "return"})`,
                );
            } finally {
                release.resolve();
                await pending;
                logSpy.mockRestore();
            }
        });
    }

    it("logs a polluted built-in prototype at warn and skips the inner hook", async () => {
        const warn = spyOn(logger.log, "warn");
        let hookCalls = 0;
        let getterCalls = 0;
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async () => {
                    hookCalls += 1;
                },
            },
        });
        const output = makeOutput();
        const saved = Object.getOwnPropertyDescriptor(Object.prototype, "agent");
        try {
            Object.defineProperty(Object.prototype, "agent", {
                configurable: true,
                get: () => {
                    getterCalls += 1;
                    return "polluted";
                },
            });
            expect(await handler({}, output)).toBeUndefined();
            expect(warn).toHaveBeenCalledWith(
                "[eidnara] transform declined: prototype_accessor at Object.prototype/agent (entry)",
            );
        } finally {
            if (saved) Object.defineProperty(Object.prototype, "agent", saved);
            else Reflect.deleteProperty(Object.prototype, "agent");
            warn.mockRestore();
        }
        expect(hookCalls).toBe(0);
        expect(getterCalls).toBe(0);
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
        });
        const output = makeOutput();
        const array = output.messages;
        expect(await handler({}, output)).toBe(array);
        expect(Object.getOwnPropertyDescriptor(array[0], "hidden")?.get).toBeDefined();
        expect(getterCalls).toBe(0);
    });

    it.each([
        "entry",
        "await",
    ])("leaves array-slot inspection at %s to the inner owner", async (window) => {
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
        });
        const output = makeOutput();
        const array = output.messages;
        const getter = () => {
            getterCalls += 1;
            throw new Error("wrapper inspected a source slot");
        };
        const pending = window === "await" ? handler({}, output) : undefined;
        if (pending) await started.promise;
        try {
            Object.defineProperty(array, "0", { get: getter });
            release.resolve();
            expect(await (pending ?? handler({}, output))).toBe(array);
            expect(hookCalls).toBe(1);
            expect(getterCalls).toBe(0);
            expect(output.messages).toBe(array);
            expect(Object.getOwnPropertyDescriptor(array, "0")?.get).toBe(getter);
        } finally {
            release.resolve();
            await pending;
        }
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
        });

        const output = makeOutput();
        const result = await handler({}, output);

        expect(result).toBe(output.messages);
        expect(result).toHaveLength(2);
        const injected = result ? result[1] : undefined;
        if (!injected) throw new Error("expected the injected message");
        expect((injected.info as { id?: string }).id).toBe("injected");
    });

    it("keeps the current host contents when the inner hook mutates then throws", async () => {
        const inserted = {
            info: { id: "inserted", role: "user", sessionID: "ses_test" },
            parts: [{ type: "text", text: "host edit before failure" }],
        } as unknown as Message;
        const handler = createMessagesTransformHandler({
            eidnara: {
                "experimental.chat.messages.transform": async (_input, out) => {
                    out.messages.push(inserted);
                    throw new Error("inner hook failed after host mutation");
                },
            },
        });

        const output = makeOutput();
        const array = output.messages;
        const original = output.messages[0];
        const result = await handler({}, output);

        expect(result).toBe(array);
        expect(output.messages).toBe(array);
        expect(result).toHaveLength(2);
        expect(result?.[0]).toBe(original);
        expect(result?.[1]).toBe(inserted);
    });

    for (const change of ["replace-member", "rebind-array"] as const) {
        it(`does not roll back a concurrent ${change} when the pending hook throws`, async () => {
            const started = Promise.withResolvers<void>();
            const release = Promise.withResolvers<void>();
            const handler = createMessagesTransformHandler({
                eidnara: {
                    "experimental.chat.messages.transform": async () => {
                        started.resolve();
                        await release.promise;
                        throw new Error("failed after concurrent host edit");
                    },
                },
            });
            const output = makeOutput();
            const originalArray = output.messages;
            const original = originalArray[0];
            const pending = handler({}, output);
            await Promise.race([started.promise, pending]);
            const replacement = makeOutput().messages[0]!;
            if (change === "rebind-array") output.messages = [replacement];
            else output.messages[0] = replacement;
            const currentArray = output.messages;
            release.resolve();
            const result = await pending;
            expect(result).toBe(currentArray);
            expect(output.messages).toBe(currentArray);
            expect(result).toHaveLength(1);
            expect(result?.[0]).toBe(replacement);
            if (change === "rebind-array") expect(originalArray[0]).toBe(original);
        });
    }

    it("no-ops when eidnara is null", async () => {
        const handler = createMessagesTransformHandler({ eidnara: null });

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
        });

        await handler({}, makeOutput());

        expect(dynamicCalled).toBe(true);
        expect(staticCalled).toBe(false);
    });
});
