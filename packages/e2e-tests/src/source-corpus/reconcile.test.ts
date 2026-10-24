import { describe, expect, it } from "bun:test";

import type { ExpectedRecord } from "./expected.ts";
import { reconcile, UNIT_CAP_BYTES } from "./reconcile.ts";

const tool = (callId: string) => ({
    name: "bash",
    callId,
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

describe("live reconciliation", () => {
    it("partitions every expected unit into admitted or one predicted loss", () => {
        const result = reconcile(
            records,
            [
                live("messages", { message_id: "m1", block_index: "0" }),
                live("raw_tool_spans", { tool_call_id: "c1", block_index: "0" }),
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
        const result = reconcile(
            records,
            [live("raw_tool_spans", { tool_call_id: "c9", block_index: "0" })],
            "s",
            () => false,
        );
        expect(result.unclassified).toEqual([
            "messages:m1:0",
            "raw_tool_spans:c1:0",
            "raw_tool_spans:c3:0",
        ]);
        expect(result.phantom).toEqual(["raw_tool_spans:c9:0"]);
    });
});
