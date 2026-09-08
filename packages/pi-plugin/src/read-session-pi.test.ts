import { describe, expect, it } from "bun:test";
import { convertEntriesToRawMessages } from "./read-session-pi";

describe("convertEntriesToRawMessages: synthetic-user entry-id propagation", () => {
    // Synthetic `RawMessage.id` values derive from the first folded `toolResult` entry ID,
    // which identifies a real `SessionEntry` and keeps every id nonempty.

    function messageEntry(id: string, message: Record<string, unknown>): Record<string, unknown> {
        return { type: "message", id, message };
    }

    it("skips current custom entries and historical ctx-status custom messages", () => {
        const entries = [
            messageEntry("user-1", { role: "user", content: "before" }),
            {
                type: "custom",
                id: "status-current",
                customType: "ctx-status",
                data: { title: "Eidnara Embed", text: "Embedding history…" },
            },
            {
                type: "custom_message",
                id: "status-historical",
                customType: "ctx-status",
                content: "Historical status",
                display: true,
            },
            messageEntry("asst-1", { role: "assistant", content: "after" }),
        ];

        expect(
            convertEntriesToRawMessages(entries).map(({ id, role }) => ({
                id,
                role,
            })),
        ).toEqual([
            { id: "user-1", role: "user" },
            { id: "asst-1", role: "assistant" },
        ]);
    });

    it("assigns the first folded toolResult's id to a synthetic user emitted at toolResult→assistant", () => {
        const entries = [
            messageEntry("user-1", { role: "user", content: "kick off" }),
            messageEntry("asst-1", {
                role: "assistant",
                content: [{ type: "toolCall", id: "tc-1", name: "read" }],
            }),
            messageEntry("tr-1", {
                role: "toolResult",
                toolCallId: "tc-1",
                toolName: "read",
                content: [{ type: "text", text: "output-1" }],
            }),
            messageEntry("asst-2", {
                role: "assistant",
                content: [{ type: "text", text: "follow-up" }],
            }),
        ];

        const raws = convertEntriesToRawMessages(entries);

        expect(raws.map((r) => ({ ordinal: r.ordinal, id: r.id, role: r.role }))).toEqual([
            { ordinal: 1, id: "user-1", role: "user" },
            { ordinal: 2, id: "asst-1", role: "assistant" },
            { ordinal: 3, id: "synth-user-tr-1", role: "user" },
            { ordinal: 4, id: "asst-2", role: "assistant" },
        ]);
    });

    it("assigns the first folded toolResult's id when multiple toolResults stack before an assistant", () => {
        const entries = [
            messageEntry("user-1", { role: "user", content: "start" }),
            messageEntry("asst-1", {
                role: "assistant",
                content: [
                    { type: "toolCall", id: "tc-1", name: "read" },
                    { type: "toolCall", id: "tc-2", name: "read" },
                ],
            }),
            messageEntry("tr-1", {
                role: "toolResult",
                toolCallId: "tc-1",
                toolName: "read",
                content: [{ type: "text", text: "out-1" }],
            }),
            messageEntry("tr-2", {
                role: "toolResult",
                toolCallId: "tc-2",
                toolName: "read",
                content: [{ type: "text", text: "out-2" }],
            }),
            // The transition folds `tr-1` and `tr-2` into one synthetic user.
            messageEntry("asst-2", { role: "assistant", content: [] }),
        ];

        const raws = convertEntriesToRawMessages(entries);
        const synthetic = raws[2];
        expect(synthetic?.ordinal).toBe(3);
        expect(synthetic?.role).toBe("user");
        expect(synthetic?.id).toBe("synth-user-tr-1");
    });

    it("names the synthetic user after the first toolResult that contributed a part", () => {
        const entries = [
            messageEntry("user-1", { role: "user", content: "start" }),
            messageEntry("asst-1", {
                role: "assistant",
                content: [{ type: "toolCall", id: "tc-2", name: "read" }],
            }),
            // No `toolCallId`: synthesis yields no parts, so this entry cannot name the turn.
            messageEntry("tr-broken", {
                role: "toolResult",
                toolName: "read",
                content: [{ type: "text", text: "orphaned" }],
            }),
            messageEntry("tr-2", {
                role: "toolResult",
                toolCallId: "tc-2",
                toolName: "read",
                content: [{ type: "text", text: "out-2" }],
            }),
            messageEntry("asst-2", { role: "assistant", content: [] }),
        ];

        const raws = convertEntriesToRawMessages(entries);
        const synthetic = raws[2];
        expect(synthetic?.id).toBe("synth-user-tr-2");
        expect(synthetic?.parts).toHaveLength(1);
    });

    it("assigns the first folded toolResult's id to a trailing-tail synthetic user", () => {
        const entries = [
            messageEntry("user-1", { role: "user", content: "start" }),
            messageEntry("asst-1", {
                role: "assistant",
                content: [{ type: "toolCall", id: "tc-1", name: "read" }],
            }),
            messageEntry("tr-tail", {
                role: "toolResult",
                toolCallId: "tc-1",
                toolName: "read",
                content: [{ type: "text", text: "tail" }],
            }),
            // A trailing `toolResult` sequence emits a synthetic user.
        ];

        const raws = convertEntriesToRawMessages(entries);
        const tail = raws[raws.length - 1];
        expect(tail?.ordinal).toBe(3);
        expect(tail?.role).toBe("user");
        expect(tail?.id).toBe("synth-user-tr-tail");
    });

    it("clears pending state after a real user folds toolResults", () => {
        // A real user message folds pending `toolResult` entries without emitting a synthetic user `RawMessage`.
        // Emission clears pending tool results so later synthetic users cannot reuse their IDs.
        const entries = [
            messageEntry("asst-1", {
                role: "assistant",
                content: [{ type: "toolCall", id: "tc-1", name: "read" }],
            }),
            messageEntry("tr-1", {
                role: "toolResult",
                toolCallId: "tc-1",
                toolName: "read",
                content: [{ type: "text", text: "x" }],
            }),
            messageEntry("real-user", { role: "user", content: "next" }),
            messageEntry("asst-2", {
                role: "assistant",
                content: [{ type: "toolCall", id: "tc-2", name: "read" }],
            }),
            messageEntry("tr-2", {
                role: "toolResult",
                toolCallId: "tc-2",
                toolName: "read",
                content: [{ type: "text", text: "y" }],
            }),
            messageEntry("asst-3", { role: "assistant", content: [] }),
        ];

        const raws = convertEntriesToRawMessages(entries);
        // Expected:
        //   1: asst-1
        //   3: asst-2
        //   5: asst-3
        expect(raws.map((r) => ({ ordinal: r.ordinal, id: r.id, role: r.role }))).toEqual([
            { ordinal: 1, id: "asst-1", role: "assistant" },
            { ordinal: 2, id: "real-user", role: "user" },
            { ordinal: 3, id: "asst-2", role: "assistant" },
            { ordinal: 4, id: "synth-user-tr-2", role: "user" },
            { ordinal: 5, id: "asst-3", role: "assistant" },
        ]);
    });

    it("reproduces the user-session ordinal divergence: every RawMessage has a non-empty id", () => {
        // Every `RawMessage` has a nonempty `id`, including synthetic users.
        //
        const entries: Array<Record<string, unknown>> = [
            messageEntry("u-0", { role: "user", content: "go" }),
        ];
        for (let i = 1; i <= 50; i++) {
            entries.push(
                messageEntry(`a-${i}`, {
                    role: "assistant",
                    content: [{ type: "toolCall", id: `tc-${i}`, name: "read" }],
                }),
                messageEntry(`tr-${i}`, {
                    role: "toolResult",
                    toolCallId: `tc-${i}`,
                    toolName: "read",
                    content: [{ type: "text", text: `out-${i}` }],
                }),
            );
        }
        entries.push(
            messageEntry("a-final", {
                role: "assistant",
                content: [{ type: "text", text: "done" }],
            }),
        );

        const raws = convertEntriesToRawMessages(entries);

        const empties = raws.filter((r) => !r.id || r.id.length === 0);
        expect(empties).toEqual([]);

        // Ordinals are contiguous from 1.
        const ordinals = raws.map((r) => r.ordinal);
        expect(ordinals[0]).toBe(1);
        for (let i = 1; i < ordinals.length; i++) {
            const prev = ordinals[i - 1] ?? 0;
            const cur = ordinals[i] ?? 0;
            expect(cur).toBe(prev + 1);
        }

        const syntheticUsers = raws.filter(
            (r, idx) =>
                r.role === "user" &&
                idx > 0 &&
                raws[idx - 1] !== undefined &&
                raws[idx - 1]?.role === "assistant" &&
                r.id.startsWith("synth-user-tr-"),
        );
        expect(syntheticUsers.length).toBe(50);
    });
});

describe("convertEntriesToRawMessages: part synthesis", () => {
    function messageEntry(id: string, message: Record<string, unknown>): Record<string, unknown> {
        return { type: "message", id, message };
    }

    it("maps assistant thinking blocks to reasoning parts alongside text and tool calls", () => {
        const raws = convertEntriesToRawMessages([
            messageEntry("asst-1", {
                role: "assistant",
                content: [
                    { type: "thinking", thinking: "plan the edit", thinkingSignature: "sig" },
                    { type: "text", text: "editing" },
                    { type: "toolCall", id: "tc-1", name: "edit", arguments: { path: "a.ts" } },
                ],
            }),
        ]);

        expect(raws[0]?.parts).toEqual([
            { type: "reasoning", text: "plan the edit" },
            { type: "text", text: "editing" },
            { type: "tool", tool: "edit", callID: "tc-1", state: { input: { path: "a.ts" } } },
        ]);
    });

    it("maps user image blocks to file parts with an image data URL", () => {
        const raws = convertEntriesToRawMessages([
            messageEntry("user-1", {
                role: "user",
                content: [
                    { type: "text", text: "what is this?" },
                    { type: "image", data: "AAAA", mimeType: "image/png" },
                ],
            }),
        ]);

        expect(raws[0]?.parts).toEqual([
            { type: "text", text: "what is this?" },
            { type: "file", mime: "image/png", url: "data:image/png;base64,AAAA" },
        ]);
    });

    it("skips image blocks that lack a string payload or mime type", () => {
        const raws = convertEntriesToRawMessages([
            messageEntry("user-1", {
                role: "user",
                content: [
                    { type: "image", mimeType: "image/png" },
                    { type: "image", data: "AAAA" },
                    { type: "text", text: "kept" },
                ],
            }),
        ]);

        expect(raws[0]?.parts).toEqual([{ type: "text", text: "kept" }]);
    });

    it("emits tool-result image blocks as file parts beside the tool part", () => {
        const raws = convertEntriesToRawMessages([
            messageEntry("asst-1", {
                role: "assistant",
                content: [{ type: "toolCall", id: "tc-1", name: "screenshot" }],
            }),
            messageEntry("tr-1", {
                role: "toolResult",
                toolCallId: "tc-1",
                toolName: "screenshot",
                content: [
                    { type: "text", text: "captured" },
                    { type: "image", data: "BBBB", mimeType: "image/jpeg" },
                    { type: "image", mimeType: "image/png" },
                ],
            }),
        ]);

        expect(raws[1]?.role).toBe("user");
        expect(raws[1]?.parts).toEqual([
            { type: "tool", tool: "screenshot", callID: "tc-1", state: { output: "captured" } },
            { type: "file", mime: "image/jpeg", url: "data:image/jpeg;base64,BBBB" },
        ]);
    });
});
