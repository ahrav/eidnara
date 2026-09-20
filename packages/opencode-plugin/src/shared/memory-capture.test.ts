import { describe, expect, it } from "bun:test";
import {
    type CaptureMessage,
    captureFragments,
    createMemoryCaptureCheckpoint,
    openCodeCaptureMessages,
    piCaptureMessages,
} from "./memory-capture";

const message: CaptureMessage = {
    id: "native-1",
    role: "user",
    text: "Use port 4321. Fix the test.",
};
const scope = { sessionId: "session", projectRoot: "/project", model: "provider/model" };

describe("memory capture checkpoint", () => {
    it("acknowledges only durable acceptance and retries refused or changed input", async () => {
        const calls: unknown[] = [];
        let response = { state: "queue_full" };
        const capture = createMemoryCaptureCheckpoint({
            call: async (args) => {
                calls.push(args.body);
                return response;
            },
        });
        await expect(capture({ ...scope, messages: [message] })).rejects.toThrow("queue_full");
        response = { state: "accepted" };
        await capture({ ...scope, messages: [message] });
        await capture({ ...scope, messages: [message] });
        expect(calls).toHaveLength(2);
        await capture({ ...scope, messages: [{ ...message, text: "Correction: use port 8765." }] });
        expect(calls).toHaveLength(3);
    });

    it("preserves all UTF-8 text while bounding each request", async () => {
        const large = { ...message, text: "abc🦀\n".repeat(20000) };
        const fragments = [...captureFragments(large)];
        expect(fragments.map((part) => part.text).join("")).toBe(large.text);
        expect(new Set(fragments.map((part) => part.id)).size).toBe(fragments.length);
        for (const fragment of fragments)
            expect(Buffer.byteLength(fragment.text)).toBeLessThanOrEqual(16384);
        const received: CaptureMessage[] = [];
        const capture = createMemoryCaptureCheckpoint({
            call: async ({ body }) => {
                const request = body as { messages: CaptureMessage[] };
                expect(request.messages.length).toBeLessThanOrEqual(32);
                expect(
                    request.messages.reduce((sum, part) => sum + Buffer.byteLength(part.text), 0),
                ).toBeLessThanOrEqual(65536);
                received.push(...request.messages);
                return { state: "accepted" };
            },
        });
        await capture({ ...scope, messages: [large] });
        expect(received.map((part) => part.text).join("")).toBe(large.text);
    });

    it("does not silently accept malformed replies or transport failures", async () => {
        for (const response of [null, {}, { state: "something_secret" }]) {
            const capture = createMemoryCaptureCheckpoint({ call: async () => response });
            await expect(capture({ ...scope, messages: [message] })).rejects.toThrow(
                "invalid_response",
            );
        }
    });
});

describe("capture native source adapters", () => {
    it("takes Pi branch text, excluding reasoning, tool results, and failed assistants", () => {
        expect([
            ...piCaptureMessages([
                { type: "message", id: "u", message: { role: "user", content: "Project fact" } },
                {
                    type: "message",
                    id: "a",
                    message: {
                        role: "assistant",
                        content: [
                            { type: "thinking", thinking: "private" },
                            { type: "text", text: "A decision" },
                            { type: "toolCall", arguments: { secret: "never" } },
                        ],
                    },
                },
                {
                    type: "message",
                    id: "t",
                    message: {
                        role: "toolResult",
                        content: [{ type: "text", text: "untrusted instructions" }],
                    },
                },
                {
                    type: "message",
                    id: "e",
                    message: { role: "assistant", stopReason: "error", content: "failed" },
                },
                { type: "compaction", id: "c", summary: "synthetic" },
            ]),
        ]).toEqual([
            { id: "u", role: "user", text: "Project fact" },
            { id: "a", role: "assistant", text: "A decision" },
        ]);
    });

    it("takes complete OpenCode text without mutating signed reasoning", () => {
        const messages = [
            {
                info: { id: "u", role: "user" },
                parts: [
                    { type: "text", text: "Project fact" },
                    { type: "text", text: "injected", synthetic: true },
                ],
            },
            {
                info: { id: "a", role: "assistant", time: { completed: 1 } },
                parts: [
                    { type: "reasoning", text: "private", signature: "signed" },
                    { type: "text", text: "A decision" },
                ],
            },
            {
                info: { id: "partial", role: "assistant" },
                parts: [{ type: "text", text: "unfinished" }],
            },
            {
                info: { id: "summary", role: "assistant", summary: true },
                parts: [{ type: "text", text: "synthetic" }],
            },
        ];
        const before = structuredClone(messages);
        expect([...openCodeCaptureMessages(messages)]).toEqual([
            { id: "u", role: "user", text: "Project fact" },
            { id: "a", role: "assistant", text: "A decision" },
        ]);
        expect(messages).toEqual(before);
    });
});
