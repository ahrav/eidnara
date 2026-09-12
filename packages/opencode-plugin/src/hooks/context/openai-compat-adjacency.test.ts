import { describe, expect, test } from "bun:test";
import { ISSUE_135_ORPHAN_WIRE } from "./issue-135-wire-fixtures";
import {
    assertOpenAiCompatAdjacency,
    type OpenAiCompatWireMessage,
} from "./openai-compat-adjacency";

describe("assertOpenAiCompatAdjacency", () => {
    test("passes valid tool_call immediately followed by tool", () => {
        const messages: OpenAiCompatWireMessage[] = [
            { role: "user", content: "go" },
            {
                role: "assistant",
                content: null,
                tool_calls: [
                    { id: "call-1", type: "function", function: { name: "read", arguments: "{}" } },
                ],
            },
            { role: "tool", tool_call_id: "call-1", content: "ok" },
        ];
        expect(assertOpenAiCompatAdjacency(messages).ok).toBe(true);
    });

    test("fails when an assistant or user message separates tool_calls from its tool result", () => {
        for (const role of ["assistant", "user"] as const) {
            const messages: OpenAiCompatWireMessage[] = [
                { role: "user", content: "go" },
                {
                    role: "assistant",
                    content: null,
                    tool_calls: [
                        {
                            id: "call-1",
                            type: "function",
                            function: { name: "read", arguments: "{}" },
                        },
                    ],
                },
                { role, content: "[dropped]" },
                { role: "tool", tool_call_id: "call-1", content: "ok" },
            ];
            const result = assertOpenAiCompatAdjacency(messages);
            expect(result.ok).toBe(false);
            expect(result.violations.map((v) => v.kind)).toEqual([
                "missing_tool_messages",
                "orphan_tool_message",
            ]);
            expect(result.violations[0]?.index).toBe(1);
        }
    });

    test("issue #135 pinned orphan fixture stays failing until fixed", () => {
        const result = assertOpenAiCompatAdjacency(ISSUE_135_ORPHAN_WIRE);
        expect(result.ok).toBe(false);
        expect(result.violations.some((v) => v.kind === "missing_tool_messages")).toBe(true);
    });

    test("fails when two tool messages answer the same tool_call_id", () => {
        const messages: OpenAiCompatWireMessage[] = [
            { role: "user", content: "go" },
            {
                role: "assistant",
                content: null,
                tool_calls: [
                    { id: "c1", type: "function", function: { name: "x", arguments: "{}" } },
                ],
            },
            { role: "tool", tool_call_id: "c1", content: "first" },
            { role: "tool", tool_call_id: "c1", content: "second" },
        ];
        const result = assertOpenAiCompatAdjacency(messages);
        expect(result.ok).toBe(false);
        expect(result.violations).toHaveLength(1);
        expect(result.violations[0]).toMatchObject({
            index: 1,
            kind: "duplicate_tool_call_id",
            toolCallId: "c1",
        });
    });

    test("fails when an assistant declares the same tool_call_id twice", () => {
        const messages: OpenAiCompatWireMessage[] = [
            {
                role: "assistant",
                content: null,
                tool_calls: [
                    { id: "c1", type: "function", function: { name: "x", arguments: "{}" } },
                    { id: "c1", type: "function", function: { name: "x", arguments: "{}" } },
                ],
            },
            { role: "tool", tool_call_id: "c1", content: "ok" },
        ];
        const result = assertOpenAiCompatAdjacency(messages);
        expect(result.ok).toBe(false);
        expect(result.violations.map((v) => v.kind)).toEqual(["duplicate_tool_call_id"]);
    });

    test("fails when a tool message without tool_call_id opens the conversation or follows a user message", () => {
        const cases: Array<{ messages: OpenAiCompatWireMessage[]; index: number }> = [
            { messages: [{ role: "tool", content: "orphan" }], index: 0 },
            {
                messages: [
                    { role: "user", content: "hi" },
                    { role: "tool", content: "orphan" },
                ],
                index: 1,
            },
        ];
        for (const { messages, index } of cases) {
            const result = assertOpenAiCompatAdjacency(messages);
            expect(result.ok).toBe(false);
            expect(result.violations).toEqual([
                expect.objectContaining({ index, kind: "orphan_tool_message" }),
            ]);
        }
    });

    test("reports a tool message without tool_call_id inside a run exactly once", () => {
        const messages: OpenAiCompatWireMessage[] = [
            {
                role: "assistant",
                content: null,
                tool_calls: [
                    { id: "c1", type: "function", function: { name: "x", arguments: "{}" } },
                ],
            },
            { role: "tool", content: "no id" },
            { role: "tool", tool_call_id: "c1", content: "ok" },
        ];
        const result = assertOpenAiCompatAdjacency(messages);
        expect(result.ok).toBe(false);
        expect(result.violations.map((v) => v.kind)).toEqual(["unmatched_tool_call_id"]);
    });
});
