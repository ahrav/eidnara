import { describe, expect, it } from "bun:test";
import {
    createMemoryCaptureDrain,
    flushMemoryCapture,
    type MemoryCaptureScope,
    NativeCaptureError,
} from "./memory-capture";

const scope = { sessionId: "s", projectRoot: "/project", model: "custom/model" };
const work = {
    state: "work",
    lease: "a".repeat(32),
    model: "custom/model",
    system: "Extract facts.",
    prompt: "A fact.",
    max_output_tokens: 8192,
    max_output_bytes: 131072,
    max_duration_ms: 90000,
};

describe("native capture exchange", () => {
    it("uses native model execution but trusts only daemon completion", async () => {
        const calls: Array<Record<string, unknown>> = [];
        const replies = [work, { state: "processed" }, { state: "ready" }];
        await flushMemoryCapture(
            {
                call: async ({ body }) => {
                    calls.push(body as Record<string, unknown>);
                    return replies.shift();
                },
            },
            scope,
            async (request) => {
                expect(request.model).toBe("custom/model");
                return { model: request.model, text: '{"version":1}' };
            },
        );
        expect(calls.map((call) => call.method)).toEqual([
            "memory.capture.next",
            "memory.capture.submit",
            "memory.capture.next",
        ]);
        expect(calls.every((call) => call.v === 2)).toBe(true);
        expect(calls[1]).toMatchObject({ lease: work.lease, output: '{"version":1}' });
    });

    it("does not turn an unconfirmed receipt or model substitution into success", async () => {
        for (const state of ["pending", "stale"]) {
            await expect(
                flushMemoryCapture({ call: async () => ({ state }) }, scope, async () => {
                    throw new Error("must not run");
                }),
            ).resolves.toBe("pending");
        }
        for (const state of ["pending", "stale"]) {
            const replies = [work, { state }];
            await expect(
                flushMemoryCapture(
                    { call: async () => replies.shift() },
                    scope,
                    async (request) => ({ model: request.model, text: "{}" }),
                ),
            ).resolves.toBe("pending");
        }
        await expect(
            flushMemoryCapture({ call: async () => ({ state: "ready" }) }, scope, async () => {
                throw new Error("must not run");
            }),
        ).resolves.toBe("ready");
        await expect(
            flushMemoryCapture({ call: async () => ({ state: "disabled" }) }, scope, async () => {
                throw new Error("must not run");
            }),
        ).resolves.toBe("disabled");
        const calls: unknown[] = [];
        await expect(
            flushMemoryCapture(
                {
                    call: async ({ body }) => {
                        calls.push(body);
                        return work;
                    },
                },
                scope,
                async () => ({ model: "other/model", text: "{}" }),
            ),
        ).rejects.toThrow("model_failed");
        expect(calls[1]).toMatchObject({ method: "memory.capture.submit", error: "model_failed" });
    });

    it("reports a submission the daemon refused as disabled, not as unfinished work", async () => {
        const replies = [work, { state: "disabled" }];
        await expect(
            flushMemoryCapture({ call: async () => replies.shift() }, scope, async (request) => ({
                model: request.model,
                text: "{}",
            })),
        ).resolves.toBe("disabled");
    });

    it("submits an answer whose executor cleanup ran past the model deadline", async () => {
        const calls: Array<Record<string, unknown>> = [];
        const replies = [
            { ...work, max_duration_ms: 20 },
            { state: "processed" },
            { state: "ready" },
        ];
        await expect(
            flushMemoryCapture(
                {
                    call: async ({ body }) => {
                        calls.push(body as Record<string, unknown>);
                        return replies.shift();
                    },
                },
                scope,
                async (request) => {
                    // The model answered within its deadline; session cleanup finished after it.
                    await new Promise<void>((resolve) => setTimeout(resolve, 40));
                    return { model: request.model, text: "{}" };
                },
            ),
        ).resolves.toBe("ready");
        expect(calls[1]).toMatchObject({ method: "memory.capture.submit", output: "{}" });
    });

    it("still fails loudly on daemon store failures and malformed replies", async () => {
        for (const state of ["store_failed", "unavailable", "something_secret"]) {
            await expect(
                flushMemoryCapture({ call: async () => ({ state }) }, scope, async () => {
                    throw new Error("must not run");
                }),
            ).rejects.toThrow("unfinished work");
            const replies = [work, { state }];
            await expect(
                flushMemoryCapture(
                    { call: async () => replies.shift() },
                    scope,
                    async (request) => ({ model: request.model, text: "{}" }),
                ),
            ).rejects.toThrow("unfinished work");
        }
        for (const response of [null, {}, "ready"]) {
            await expect(
                flushMemoryCapture({ call: async () => response }, scope, async () => {
                    throw new Error("must not run");
                }),
            ).rejects.toThrow("unfinished work");
        }
    });

    it("accepts daemon bounds at or below the harness ceiling and passes them through", async () => {
        const replies = [
            { ...work, max_output_tokens: 4096, max_output_bytes: 65536, max_duration_ms: 30000 },
            { state: "processed" },
            { state: "ready" },
        ];
        const seen: unknown[] = [];
        await expect(
            flushMemoryCapture({ call: async () => replies.shift() }, scope, async (request) => {
                seen.push(request);
                return { model: request.model, text: "{}" };
            }),
        ).resolves.toBe("ready");
        expect(seen).toEqual([
            {
                model: "custom/model",
                system: "Extract facts.",
                prompt: "A fact.",
                maxOutputTokens: 4096,
                maxOutputBytes: 65536,
                maxDurationMs: 30000,
            },
        ]);
        for (const overrides of [
            { max_output_tokens: 8193 },
            { max_output_bytes: 131073 },
            { max_duration_ms: 90001 },
            { max_output_tokens: 0 },
            { max_output_bytes: -1 },
            { max_duration_ms: 1.5 },
        ]) {
            const calls: Array<Record<string, unknown>> = [];
            await expect(
                flushMemoryCapture(
                    {
                        call: async ({ body }) => {
                            calls.push(body as Record<string, unknown>);
                            return { ...work, ...overrides };
                        },
                    },
                    scope,
                    async () => {
                        throw new Error("must not run");
                    },
                ),
            ).rejects.toThrow("Invalid native capture work response");
            expect(calls.map((call) => call.method)).toEqual([
                "memory.capture.next",
                "memory.capture.submit",
            ]);
            expect(calls[1]).toMatchObject({ lease: work.lease, error: "cancelled" });
        }
    });

    it("releases the lease of a malformed work response before failing", async () => {
        const calls: Array<Record<string, unknown>> = [];
        const { prompt: _prompt, ...withoutPrompt } = work;
        await expect(
            flushMemoryCapture(
                {
                    call: async ({ body }) => {
                        calls.push(body as Record<string, unknown>);
                        return withoutPrompt;
                    },
                },
                scope,
                async () => {
                    throw new Error("must not run");
                },
            ),
        ).rejects.toThrow("Invalid native capture work response");
        expect(calls).toHaveLength(2);
        expect(calls[1]).toMatchObject({
            method: "memory.capture.submit",
            lease: work.lease,
            error: "cancelled",
        });
        expect(calls[1]).not.toHaveProperty("output");
        // A failing release still surfaces the malformed response, not the release failure.
        const failing = [work.lease, "not-a-lease"].map((lease) => ({
            ...withoutPrompt,
            lease,
        }));
        for (const response of failing) {
            const attempted: Array<Record<string, unknown>> = [];
            await expect(
                flushMemoryCapture(
                    {
                        call: async ({ body }) => {
                            const request = body as Record<string, unknown>;
                            attempted.push(request);
                            if (request.method === "memory.capture.submit")
                                throw new Error("transport down");
                            return response;
                        },
                    },
                    scope,
                    async () => {
                        throw new Error("must not run");
                    },
                ),
            ).rejects.toThrow("Invalid native capture work response");
            expect(
                attempted.filter((call) => call.method === "memory.capture.submit"),
            ).toHaveLength(response.lease === work.lease ? 1 : 0);
        }
    });

    it("never forwards native authentication errors or excessive output", async () => {
        for (const failure of [
            new Error("secret-access-token"),
            new NativeCaptureError("provider_unavailable"),
        ]) {
            const calls: unknown[] = [];
            await expect(
                flushMemoryCapture(
                    {
                        call: async ({ body }) => {
                            calls.push(body);
                            return work;
                        },
                    },
                    scope,
                    async () => {
                        throw failure;
                    },
                ),
            ).rejects.toThrow("Native memory capture:");
            expect(JSON.stringify(calls)).not.toContain("secret-access-token");
        }
        const calls: unknown[] = [];
        await expect(
            flushMemoryCapture(
                {
                    call: async ({ body }) => {
                        calls.push(body);
                        return work;
                    },
                },
                scope,
                async () => ({ model: work.model, text: "x".repeat(work.max_output_bytes + 1) }),
            ),
        ).rejects.toThrow("output_limit");
        expect(calls[1]).toMatchObject({ error: "output_limit" });
        expect(JSON.stringify(calls).length).toBeLessThan(1000);
    });
});

interface Deferred {
    projectRoot: string;
    resolve(response: unknown): void;
}

/** A daemon stand-in whose `memory.capture.next` replies are released by the test. */
function controlledDaemon() {
    const waiting: Deferred[] = [];
    const drained: string[] = [];
    return {
        waiting,
        drained,
        client: {
            call: ({ projectRoot }: { projectRoot: string }) =>
                new Promise<unknown>((resolve) => {
                    drained.push(projectRoot);
                    waiting.push({ projectRoot, resolve });
                }),
        },
    };
}

const noExecutor = async () => {
    throw new Error("must not run");
};

async function tick(): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("memory capture drain", () => {
    it("coalesces schedules for one project into a single rerun", async () => {
        const daemon = controlledDaemon();
        const settled: Array<[MemoryCaptureScope, string]> = [];
        const failed: unknown[] = [];
        const drain = createMemoryCaptureDrain(daemon.client, noExecutor, {
            onSettled: (scope, result) => settled.push([scope, result]),
            onFailed: (_scope, error) => failed.push(error),
        });
        expect(drain.pending("/project")).toBeUndefined();
        drain.schedule({ ...scope, sessionId: "first" });
        drain.schedule({ ...scope, sessionId: "second" });
        drain.schedule({ ...scope, sessionId: "third" });
        await tick();
        expect(daemon.drained).toEqual(["/project"]);
        const pending = drain.pending("/project");
        expect(pending).toBeDefined();
        let resolved = false;
        void pending?.then(() => {
            resolved = true;
        });
        daemon.waiting.shift()?.resolve({ state: "ready" });
        await tick();
        expect(resolved).toBe(false);
        expect(daemon.drained).toEqual(["/project", "/project"]);
        drain.schedule({ ...scope, sessionId: "fourth" });
        daemon.waiting.shift()?.resolve({ state: "pending" });
        await tick();
        expect(resolved).toBe(false);
        expect(daemon.drained).toEqual(["/project", "/project", "/project"]);
        daemon.waiting.shift()?.resolve({ state: "disabled" });
        await pending;
        expect(resolved).toBe(true);
        await drain.settle();
        expect(daemon.drained).toHaveLength(3);
        expect(settled.map(([passScope, result]) => [passScope.sessionId, result])).toEqual([
            ["first", "ready"],
            ["third", "pending"],
            ["fourth", "disabled"],
        ]);
        expect(failed).toEqual([]);
        expect(drain.pending("/project")).toBeUndefined();
    });

    it("drains different projects concurrently", async () => {
        const daemon = controlledDaemon();
        const settled: string[] = [];
        const drain = createMemoryCaptureDrain(daemon.client, noExecutor, {
            onSettled: (passScope) => settled.push(passScope.projectRoot),
            onFailed: () => {
                throw new Error("must not fail");
            },
        });
        drain.schedule({ ...scope, projectRoot: "/a" });
        drain.schedule({ ...scope, projectRoot: "/b" });
        await tick();
        expect(daemon.drained).toEqual(["/a", "/b"]);
        expect(daemon.waiting).toHaveLength(2);
        daemon.waiting.pop()?.resolve({ state: "ready" });
        await drain.pending("/b");
        expect(settled).toEqual(["/b"]);
        expect(drain.pending("/a")).toBeDefined();
        daemon.waiting.pop()?.resolve({ state: "ready" });
        await drain.settle();
        expect(settled).toEqual(["/b", "/a"]);
    });

    it("reports thrown failures without blocking the next drain or other projects", async () => {
        const daemon = controlledDaemon();
        const failed: Array<[string, unknown]> = [];
        const settled: string[] = [];
        const drain = createMemoryCaptureDrain(daemon.client, noExecutor, {
            onSettled: (passScope, result) => {
                settled.push(`${passScope.projectRoot}:${result}`);
                throw new Error("hook bug");
            },
            onFailed: (passScope, error) => {
                failed.push([passScope.projectRoot, error]);
                throw new Error("hook bug");
            },
        });
        drain.schedule({ ...scope, projectRoot: "/a" });
        drain.schedule({ ...scope, projectRoot: "/b" });
        await tick();
        daemon.waiting.shift()?.resolve({ state: "store_failed" });
        await drain.pending("/a");
        expect(failed).toHaveLength(1);
        expect(failed[0]?.[0]).toBe("/a");
        expect(failed[0]?.[1]).toBeInstanceOf(Error);
        expect(drain.pending("/a")).toBeUndefined();
        drain.schedule({ ...scope, projectRoot: "/a" });
        await tick();
        expect(daemon.drained).toEqual(["/a", "/b", "/a"]);
        daemon.waiting.shift()?.resolve({ state: "ready" });
        daemon.waiting.shift()?.resolve({ state: "ready" });
        await drain.settle();
        expect(settled.sort()).toEqual(["/a:ready", "/b:ready"]);
        expect(failed).toHaveLength(1);
    });
});
