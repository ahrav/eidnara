import { describe, expect, it } from "bun:test";
import {
    CAPTURE_MAX_AGE_MS,
    type CaptureMessage,
    captureFragments,
    createMemoryCaptureCheckpoint,
    openCodeCaptureMessages,
    openCodeLastFinalMessageId,
    openCodeMessagesSince,
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

    it("treats a disabled checkpoint as a quiet no-op that acknowledges nothing", async () => {
        const calls: unknown[] = [];
        let response = { state: "disabled" };
        const capture = createMemoryCaptureCheckpoint({
            call: async (args) => {
                calls.push(args.body);
                return response;
            },
        });
        await expect(capture({ ...scope, messages: [message] })).resolves.toBe("disabled");
        response = { state: "accepted" };
        await expect(capture({ ...scope, messages: [message] })).resolves.toBe("accepted");
        expect(calls).toHaveLength(2);
    });
    it("stops before the next batch once the caller says to", async () => {
        const calls: unknown[] = [];
        const capture = createMemoryCaptureCheckpoint({
            call: async (args) => {
                calls.push(args.body);
                return { state: "accepted" };
            },
        });
        const large = { ...message, text: "abc\u{1F980}\n".repeat(20000) };
        await expect(
            capture({ ...scope, messages: [large], stop: () => calls.length >= 1 }),
        ).resolves.toBe("stopped");
        expect(calls).toHaveLength(1);
    });

    it("stops batching after the first disabled reply", async () => {
        const calls: unknown[] = [];
        const capture = createMemoryCaptureCheckpoint({
            call: async (args) => {
                calls.push(args.body);
                return { state: "disabled" };
            },
        });
        const large = { ...message, text: "abc\u{1F980}\n".repeat(20000) };
        expect(Buffer.byteLength(large.text)).toBeGreaterThan(64 * 1024);
        await expect(capture({ ...scope, messages: [large] })).resolves.toBe("disabled");
        expect(calls).toHaveLength(1);
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

    it("skips Pi entries older than notBefore and keeps undated ones", () => {
        const entries = [
            {
                type: "message",
                id: "old",
                timestamp: "2026-09-18T12:00:00.000Z",
                message: { role: "user", content: "Stale fact" },
            },
            {
                type: "message",
                id: "recent",
                timestamp: "2026-09-20T12:00:00.000Z",
                message: { role: "user", content: "Fresh fact" },
            },
            {
                type: "message",
                id: "message-time",
                message: { role: "user", content: "Older by message clock", timestamp: 1 },
            },
            {
                type: "message",
                id: "entry-wins",
                timestamp: "2026-09-20T12:00:00.000Z",
                message: { role: "user", content: "Entry clock wins", timestamp: 1 },
            },
            {
                type: "message",
                id: "garbled",
                timestamp: "not a date",
                message: { role: "user", content: "Age unknown" },
            },
            { type: "message", id: "undated", message: { role: "user", content: "Undated" } },
        ];
        const notBefore = Date.parse("2026-09-19T00:00:00.000Z");
        expect([...piCaptureMessages(entries, { notBefore })].map((part) => part.id)).toEqual([
            "recent",
            "entry-wins",
            "garbled",
            "undated",
        ]);
        expect([...piCaptureMessages(entries)].map((part) => part.id)).toEqual([
            "old",
            "recent",
            "message-time",
            "entry-wins",
            "garbled",
            "undated",
        ]);
        expect([...piCaptureMessages(entries, {})].map((part) => part.id)).toHaveLength(6);
    });

    it("skips OpenCode messages older than notBefore and keeps undated ones", () => {
        const notBefore = Date.parse("2026-09-19T00:00:00.000Z");
        const messages = [
            {
                info: { id: "old", role: "user", time: { created: notBefore - 1 } },
                parts: [{ type: "text", text: "Stale fact" }],
            },
            {
                info: { id: "boundary", role: "user", time: { created: notBefore } },
                parts: [{ type: "text", text: "At the boundary" }],
            },
            {
                info: { id: "snake", role: "user", time_created: notBefore - 1 },
                parts: [{ type: "text", text: "Stale via time_created" }],
            },
            {
                info: { id: "camel", role: "user", timeCreated: notBefore + 1 },
                parts: [{ type: "text", text: "Fresh via timeCreated" }],
            },
            {
                info: { id: "assistant", role: "assistant", time: { completed: notBefore - 1 } },
                parts: [{ type: "text", text: "Completed but undated creation" }],
            },
            {
                info: { id: "undated", role: "user", time: { created: "yesterday" } },
                parts: [{ type: "text", text: "Undated" }],
            },
        ];
        expect(
            [...openCodeCaptureMessages(messages, { notBefore })].map((part) => part.id),
        ).toEqual(["boundary", "camel", "assistant", "undated"]);
        expect([...openCodeCaptureMessages(messages)].map((part) => part.id)).toEqual([
            "old",
            "boundary",
            "snake",
            "camel",
            "assistant",
            "undated",
        ]);
        expect(CAPTURE_MAX_AGE_MS).toBe(48 * 60 * 60 * 1000);
    });

    it("advances the transcript watermark only past final messages and slices tails after it", () => {
        const user = { info: { id: "u1", role: "user" }, parts: [] };
        const done = { info: { id: "a1", role: "assistant", time: { completed: 5 } }, parts: [] };
        const failed = { info: { id: "a2", role: "assistant", error: { name: "x" } }, parts: [] };
        const running = { info: { id: "a3", role: "assistant", time: { created: 6 } }, parts: [] };
        expect(openCodeLastFinalMessageId([user, done, running])).toBe("a1");
        expect(openCodeLastFinalMessageId([user, failed])).toBe("a2");
        expect(openCodeLastFinalMessageId([running])).toBeUndefined();
        expect(openCodeLastFinalMessageId(["garbled", null])).toBeUndefined();
        const tail = [user, done, failed, running];
        expect(openCodeMessagesSince(tail, "a1", 4)).toEqual([failed, running]);
        expect(openCodeMessagesSince(tail, "a3", 4)).toEqual([]);
        // A full-length tail without the watermark means it fell off; a shorter one is the whole transcript.
        expect(openCodeMessagesSince(tail, "gone", 4)).toBeUndefined();
        expect(openCodeMessagesSince(tail, "gone", 8)).toEqual(tail);
        expect(openCodeMessagesSince(tail, "gone", 2)).toEqual(tail);
    });
});
