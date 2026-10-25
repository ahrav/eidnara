import { describe, expect, it } from "bun:test";

import type { ExpectedRecord } from "./expected.ts";
import { reconcile, UNIT_CAP_BYTES } from "./reconcile.ts";

const tool = (callId: string, parentMessageId = "m2") => ({
    name: "bash",
    callId,
    parentMessageId,
    isError: false,
    exit: 0,
    truncated: false,
});
const records: ExpectedRecord[] = [
    {
        id: "m1",
        role: "user",
        units: [{ index: 0, partId: "p1", block: 0, byteLen: 4, tool: null }],
        nonUnits: [],
    },
    {
        id: "m2",
        role: "assistant",
        units: [
            { index: 1, partId: "p2", block: 0, byteLen: 10, tool: tool("c1") },
            { index: 2, partId: "p3", block: 0, byteLen: UNIT_CAP_BYTES + 1, tool: tool("c2") },
            { index: 3, partId: "p4", block: 0, byteLen: 40, tool: tool("c3") },
        ],
        nonUnits: [],
    },
];
const live = (cls: string, identity: Record<string, string>) => ({
    class: cls,
    identity: { session_id: "s", ...identity },
});
const span = (callId: string, parent = "m2", revision = "1") =>
    live("raw_tool_spans", {
        parent_message_id: parent,
        tool_call_id: callId,
        result_revision: revision,
        block_index: "0",
    });

describe("live reconciliation", () => {
    it("partitions every expected unit into admitted or one predicted loss", () => {
        const result = reconcile(
            records,
            [
                live("messages", { message_id: "m1", block_index: "0" }),
                span("c1"),
                live("messages", { message_id: "other", block_index: "0", session_id: "x" }),
            ],
            "s",
            (_, unit) => unit.tool?.callId === "c3",
        );
        expect(result).toEqual({
            expected: 4,
            admitted: 2,
            losses: { unit_too_large: 1, secret_bearing: 1 },
            unclassified: [],
            phantom: [],
        });
    });

    it("names a dropped unit and a descriptor no expected unit names", () => {
        const result = reconcile(records, [span("c9")], "s", () => false);
        expect(result.unclassified).toEqual([
            "messages:m1:0",
            "raw_tool_spans:m2:c1:0",
            "raw_tool_spans:m2:c3:0",
        ]);
        expect(result.phantom).toEqual(["raw_tool_spans:m2:c9:0"]);
    });

    it("does not admit a tool span published under another parent message", () => {
        const result = reconcile(records.slice(1, 2), [span("c1", "m9")], "s", () => false);
        expect(result.admitted).toBe(0);
        expect(result.unclassified).toEqual(["raw_tool_spans:m2:c1:0", "raw_tool_spans:m2:c3:0"]);
        expect(result.phantom).toEqual(["raw_tool_spans:m9:c1:0"]);
    });

    it("lets one descriptor admit one expected unit", () => {
        const twoRevisions = reconcile(
            records.slice(1, 2),
            [span("c1", "m2", "1"), span("c1", "m2", "2"), span("c3")],
            "s",
            () => false,
        );
        expect(twoRevisions.admitted).toBe(2);
        expect(twoRevisions.phantom).toEqual(["raw_tool_spans:m2:c1:0"]);
        const shared: ExpectedRecord = {
            id: "m3",
            role: "assistant",
            units: [
                { index: 0, partId: "p5", block: 0, byteLen: 4, tool: tool("c5", "m3") },
                { index: 1, partId: "p6", block: 0, byteLen: 4, tool: tool("c5", "m3") },
            ],
            nonUnits: [],
        };
        const oneDescriptor = reconcile([shared], [span("c5", "m3")], "s", () => false);
        expect(oneDescriptor.admitted).toBe(1);
        expect(oneDescriptor.unclassified).toEqual(["raw_tool_spans:m3:c5:0"]);
    });
});
