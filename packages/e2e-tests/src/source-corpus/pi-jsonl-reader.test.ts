import { describe, expect, it } from "bun:test";

import { readPiSession } from "./pi-jsonl-reader.ts";

const entry = (id: string, message: object) =>
    JSON.stringify({ type: "message", id, parentId: "p", timestamp: "t", message });
const image = { type: "image", data: "AAAA", mimeType: "image/png" };

describe("Pi JSONL reader", () => {
    it("classifies an image-bearing toolResult whole as non_text_item", () => {
        const jsonl = [
            JSON.stringify({ type: "session", id: "s", version: 3, cwd: "/w" }),
            entry("u1", { role: "user", content: [{ type: "text", text: "look" }, image] }),
            entry("t1", {
                role: "toolResult",
                toolCallId: "c1",
                toolName: "read",
                isError: false,
                content: [{ type: "text", text: "pixels" }, image],
            }),
            entry("t2", {
                role: "toolResult",
                toolCallId: "c2",
                toolName: "bash",
                isError: true,
                content: [{ type: "text", text: "é" }],
                details: { truncation: { truncated: true } },
            }),
            entry("u2", { role: "user", content: "plain" }),
            entry("x1", { role: "bashExecution", command: "ls" }),
        ].join("\n");
        const records = readPiSession(jsonl);
        expect(records[0]?.nonUnits.map((n) => n.reason)).toEqual(["non_message_entry"]);
        expect(records[1]?.units.map((u) => [u.index, u.byteLen])).toEqual([[0, 4]]);
        expect(records[1]?.nonUnits.map((n) => [n.index, n.reason])).toEqual([[1, "skipped_item"]]);
        expect(records[2]?.units).toEqual([]);
        expect(records[2]?.nonUnits.map((n) => [n.reason, n.kind])).toEqual([
            ["non_text_item", "image"],
        ]);
        expect(records[3]?.units[0]?.tool).toEqual({
            name: "bash",
            callId: "c2",
            parentMessageId: "p",
            isError: true,
            exit: null,
            truncated: true,
        });
        expect(records[3]?.units[0]?.byteLen).toBe(2);
        expect(records[4]?.units.map((u) => u.byteLen)).toEqual([5]);
        expect(records[5]?.nonUnits.map((n) => [n.reason, n.kind])).toEqual([
            ["uncaptured_role", "bashExecution"],
        ]);
    });

    it("refuses an entry missing a field Pi always writes", () => {
        expect(() => readPiSession("{not json")).toThrow(/line 1/);
        expect(() =>
            readPiSession(
                entry("t", { role: "toolResult", toolCallId: "c", toolName: "read", content: [] }),
            ),
        ).toThrow(/isError/);
    });
});
