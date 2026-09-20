import { describe, expect, it } from "bun:test";
import { flushMemoryCapture, NativeCaptureError } from "./memory-capture";

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
        await expect(
            flushMemoryCapture({ call: async () => ({ state: "pending" }) }, scope, async () => {
                throw new Error("must not run");
            }),
        ).rejects.toThrow("unfinished work");
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
