/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";

import type { RawMessage } from "./read-session-raw";
import type { ProviderShapeVersion } from "./read-session-true-raw-tokens";
import {
    buildToolArcs,
    buildTrueRawTokenIndex,
    buildTrueRawTokenIndexFromTokenCountsForTest,
    completedToolArcCrossesBoundary,
    computeRawRangeFingerprint,
    estimateTrueRawMessageTokens,
    fenceBoundaryForToolArcs,
    invalidateTrueRawTokenCache,
} from "./read-session-true-raw-tokens";

function singlePartMessage(part: unknown, ordinal = 1): RawMessage {
    return { id: `m-${ordinal}`, role: "user", parts: [part], ordinal };
}

function fingerprintOf(part: unknown, shape: ProviderShapeVersion = "opencode-v1"): string {
    return computeRawRangeFingerprint([singlePartMessage(part)], 1, 2, shape);
}

describe("true raw token indexes with continued ordinals", () => {
    it("maps token queries relative to the first absolute ordinal", () => {
        const messages: RawMessage[] = [
            { id: "summary", role: "user", parts: [], ordinal: 101 },
            { id: "tail", role: "assistant", parts: [], ordinal: 102 },
        ];
        const totals = new Map([
            ["summary", 10],
            ["tail", 20],
        ]);
        const index = buildTrueRawTokenIndex("continued", messages, {
            providerShapeVersion: "opencode-v1",
            cacheNamespace: "continued-test",
            absoluteMessageCount: 102,
            storedTotalForMessage: (message) => totals.get(message.id) ?? null,
        });

        expect(index.rawMessageCount).toBe(102);
        expect(index.tokenForOrdinal(1)).toBe(0);
        expect(index.tokenForOrdinal(101)).toBe(10);
        expect(index.tokenForOrdinal(102)).toBe(20);
        expect(index.messageIdAtOrdinal(101)).toBe("summary");
        expect(index.suffixTokensFromOrdinal(101)).toBe(30);
        expect(index.rangeTokens(101, 103)).toBe(30);
        expect(index.findSuffixStartForTokens(20)).toBe(102);
        expect(index.findHeadEndForCap(101, 103, 10)).toBe(102);
    });

    it("returns zero tokens for ranges that start beyond the represented tail", () => {
        const messages: RawMessage[] = [
            { id: "a", role: "user", parts: [], ordinal: 101 },
            { id: "b", role: "assistant", parts: [], ordinal: 102 },
        ];
        const totals = new Map([
            ["a", 10],
            ["b", 20],
        ]);
        const index = buildTrueRawTokenIndex("continued", messages, {
            providerShapeVersion: "opencode-v1",
            cacheNamespace: "range-clamp-test",
            absoluteMessageCount: 102,
            storedTotalForMessage: (message) => totals.get(message.id) ?? null,
        });

        expect(index.rangeTokens(103, 110)).toBe(0);
        expect(index.rangeTokens(105, 110)).toBe(0);
        expect(index.rangeTokens(102, 110)).toBe(20);

        const empty = buildTrueRawTokenIndex("empty", [], {
            providerShapeVersion: "opencode-v1",
            cacheNamespace: "range-clamp-empty-test",
        });
        expect(empty.rangeTokens(3, 5)).toBe(0);

        const fixture = buildTrueRawTokenIndexFromTokenCountsForTest("fixture", [1, 2, 3]);
        expect(fixture.rangeTokens(6, 8)).toBe(0);
        expect(fixture.rangeTokens(1, 4)).toBe(6);
    });

    it("advances a head cap past an ordinal hole so the head always holds a message", () => {
        const messages: RawMessage[] = [
            { id: "a", role: "user", parts: [], ordinal: 101 },
            { id: "c", role: "assistant", parts: [], ordinal: 103 },
        ];
        const totals = new Map([
            ["a", 10],
            ["c", 5000],
        ]);
        const index = buildTrueRawTokenIndex("holes", messages, {
            providerShapeVersion: "opencode-v1",
            cacheNamespace: "hole-test",
            absoluteMessageCount: 103,
            storedTotalForMessage: (message) => totals.get(message.id) ?? null,
        });

        expect(index.findHeadEndForCap(102, 104, 100)).toBe(104);
        expect(index.findHeadEndForCap(101, 104, 5)).toBe(102);
        expect(index.rangeTokens(101, index.findHeadEndForCap(101, 104, 100))).toBe(10);
    });

    it("ends a head cap right after the last fitting message, not after a trailing hole", () => {
        const messages: RawMessage[] = [
            { id: "a", role: "user", parts: [], ordinal: 7 },
            { id: "b", role: "user", parts: [], ordinal: 9 },
        ];
        const totals = new Map([
            ["a", 13],
            ["b", 15],
        ]);
        const index = buildTrueRawTokenIndex("trailing-hole", messages, {
            providerShapeVersion: "opencode-v1",
            cacheNamespace: "trailing-hole-test",
            absoluteMessageCount: 9,
            storedTotalForMessage: (message) => totals.get(message.id) ?? null,
        });

        const end = index.findHeadEndForCap(7, 10, 24);
        expect(end).toBe(8);
        expect(index.messageIdAtOrdinal(end - 1)).toBe("a");
        expect(index.findHeadEndForCap(7, 10, 100)).toBe(10);
    });
});

describe("tool arcs", () => {
    it("closes a tool_use arc through the tool_result's tool_use_id", () => {
        const messages: RawMessage[] = [
            {
                id: "call",
                role: "assistant",
                parts: [{ type: "tool_use", id: "toolu_1", name: "bash", input: { cmd: "ls" } }],
                ordinal: 1,
            },
            {
                id: "result",
                role: "user",
                parts: [{ type: "tool_result", tool_use_id: "toolu_1", content: "file.txt" }],
                ordinal: 2,
            },
        ];

        expect(buildToolArcs(messages, "opencode-v1")).toEqual([
            { callId: "toolu_1", invOrdinal: 1, resOrdinal: 2 },
        ]);

        const breakdown = estimateTrueRawMessageTokens(messages[1], {
            providerShapeVersion: "opencode-v1",
        });
        expect(breakdown.toolOutput).toBeGreaterThan(0);
        expect(breakdown.other).toBe(0);
    });

    it("never opens an arc for a provider-executed tool while still counting its input", () => {
        const message: RawMessage = {
            id: "provider-tool",
            role: "assistant",
            parts: [
                {
                    type: "tool",
                    callID: "c1",
                    providerExecuted: true,
                    state: { status: "running", input: { query: "weather" } },
                },
            ],
            ordinal: 1,
        };

        expect(buildToolArcs([message], "opencode-v1")).toEqual([]);

        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
        });
        expect(breakdown.toolInput).toBeGreaterThan(0);
    });

    it("treats a tool-invocation carrying an inline result as a completed call", () => {
        const message: RawMessage = {
            id: "invocation",
            role: "assistant",
            parts: [
                {
                    type: "tool-invocation",
                    toolCallId: "call_1",
                    toolName: "bash",
                    args: { cmd: "ls" },
                    result: "file.txt\nother.txt",
                },
            ],
            ordinal: 1,
        };

        expect(buildToolArcs([message], "opencode-v1")).toEqual([
            { callId: "call_1", invOrdinal: 1, resOrdinal: 1 },
        ]);

        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
        });
        expect(breakdown.toolInput).toBeGreaterThan(0);
        expect(breakdown.toolOutput).toBeGreaterThan(0);
    });

    it("treats an explicit null tool-invocation result as a completed call", () => {
        const message: RawMessage = {
            id: "invocation",
            role: "assistant",
            parts: [{ type: "tool-invocation", toolCallId: "call_1", args: {}, result: null }],
            ordinal: 1,
        };

        expect(buildToolArcs([message], "opencode-v1")).toEqual([
            { callId: "call_1", invOrdinal: 1, resOrdinal: 1 },
        ]);
    });

    it("reads providerExecuted from tool metadata", () => {
        const message: RawMessage = {
            id: "provider-tool",
            role: "assistant",
            parts: [
                {
                    type: "tool",
                    callID: "c1",
                    tool: "web",
                    metadata: { providerExecuted: true },
                    state: { status: "running", input: { q: 1 } },
                },
            ],
            ordinal: 1,
        };
        expect(buildToolArcs([message], "opencode-v1")).toEqual([]);
    });

    it("reads top-level input and output fields on OpenCode tool parts", () => {
        const message: RawMessage = {
            id: "flat-tool",
            role: "assistant",
            parts: [
                {
                    type: "tool",
                    callID: "c1",
                    tool: "bash",
                    input: { cmd: "ls -la" },
                    output: "a\nb\nc",
                    status: "completed",
                },
            ],
            ordinal: 1,
        };
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
        });
        expect(breakdown.toolInput).toBeGreaterThan(0);
        expect(breakdown.toolOutput).toBeGreaterThan(0);
        expect(buildToolArcs([message], "opencode-v1")).toEqual([
            { callId: "c1", invOrdinal: 1, resOrdinal: 1 },
        ]);
    });

    it("closes an arc on a terminal status even without an output payload", () => {
        const messages: RawMessage[] = [
            {
                id: "completed",
                role: "assistant",
                parts: [{ type: "tool", callID: "c1", state: { status: "completed", input: {} } }],
                ordinal: 1,
            },
            {
                id: "errored",
                role: "assistant",
                parts: [{ type: "tool", callID: "c2", state: { status: "error", input: {} } }],
                ordinal: 2,
            },
            {
                id: "running",
                role: "assistant",
                parts: [{ type: "tool", callID: "c3", state: { status: "running", input: {} } }],
                ordinal: 3,
            },
        ];
        expect(buildToolArcs(messages, "opencode-v1")).toEqual([
            { callId: "c1", invOrdinal: 1, resOrdinal: 1 },
            { callId: "c2", invOrdinal: 2, resOrdinal: 2 },
            { callId: "c3", invOrdinal: 3, resOrdinal: null },
        ]);
    });

    it("synthesizes distinct identities for adjacent idless OpenCode tools", () => {
        const message: RawMessage = {
            id: "idless",
            role: "assistant",
            parts: [
                { type: "tool", tool: "read", state: { status: "running", input: { path: "a" } } },
                { type: "tool", tool: "read", state: { status: "running", input: { path: "b" } } },
            ],
            ordinal: 7,
        };
        const arcs = buildToolArcs([message], "opencode-v1");
        expect(arcs).toHaveLength(2);
        expect(arcs[0].callId).not.toBe(arcs[1].callId);
        expect(arcs.every((arc) => arc.invOrdinal === 7 && arc.resOrdinal === null)).toBe(true);
    });

    it("recognizes a Pi toolCall block as an open invocation with its arguments as input", () => {
        const message: RawMessage = {
            id: "pi-call",
            role: "assistant",
            parts: [
                { type: "toolCall", id: "tc1", name: "bash", arguments: { cmd: "ls -la /tmp" } },
            ],
            ordinal: 1,
        };
        expect(buildToolArcs([message], "pi-folded-v1")).toEqual([
            { callId: "tc1", invOrdinal: 1, resOrdinal: null },
        ]);
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "pi-folded-v1",
        });
        expect(breakdown.toolInput).toBeGreaterThan(0);
        expect(breakdown.other).toBe(0);
    });

    it("synthesizes an identity for an idless Pi toolCall", () => {
        const message: RawMessage = {
            id: "pi-idless",
            role: "assistant",
            parts: [{ type: "toolCall", name: "bash", arguments: { cmd: "ls" } }],
            ordinal: 1,
        };
        const arcs = buildToolArcs([message], "pi-folded-v1");
        expect(arcs).toHaveLength(1);
        expect(arcs[0].resOrdinal).toBeNull();
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "pi-folded-v1",
        });
        expect(breakdown.toolInput).toBeGreaterThan(0);
        expect(breakdown.other).toBe(0);
    });

    it("closes a Pi toolCall through a folded toolResult part and counts its media", () => {
        const call: RawMessage = {
            id: "call",
            role: "assistant",
            parts: [{ type: "toolCall", id: "tc1", name: "read", arguments: {} }],
            ordinal: 1,
        };
        const folded: RawMessage = {
            id: "folded",
            role: "user",
            parts: [
                {
                    role: "toolResult",
                    toolCallId: "tc1",
                    toolName: "read",
                    content: [
                        { type: "text", text: "ok" },
                        { type: "image", mimeType: "image/png", data: "A".repeat(20_000) },
                    ],
                },
            ],
            ordinal: 2,
        };
        expect(buildToolArcs([call, folded], "pi-folded-v1")).toEqual([
            { callId: "tc1", invOrdinal: 1, resOrdinal: 2 },
        ]);
        const breakdown = estimateTrueRawMessageTokens(folded, {
            providerShapeVersion: "pi-folded-v1",
            imageTokenHeuristic: () => 300,
        });
        expect(breakdown.toolOutput).toBeLessThan(10);
        expect(breakdown.image).toBe(300);
        expect(breakdown.other).toBe(0);
    });

    it("keeps an argument-less Pi toolCall open", () => {
        const message: RawMessage = {
            id: "argless",
            role: "assistant",
            parts: [{ type: "toolCall", id: "tc1", name: "noop" }],
            ordinal: 1,
        };
        expect(buildToolArcs([message], "pi-folded-v1")).toEqual([
            { callId: "tc1", invOrdinal: 1, resOrdinal: null },
        ]);
    });

    it("counts a folded Pi toolResult without a toolCallId as tool output", () => {
        const message: RawMessage = {
            id: "folded-idless",
            role: "user",
            parts: [
                {
                    role: "toolResult",
                    toolName: "read",
                    content: [{ type: "image", mimeType: "image/png", data: "A".repeat(20_000) }],
                },
            ],
            ordinal: 2,
        };
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "pi-folded-v1",
            imageTokenHeuristic: () => 300,
        });
        expect(breakdown.image).toBe(300);
        expect(breakdown.other).toBe(0);
    });

    it("closes an arc with a payload-less folded Pi toolResult", () => {
        const call: RawMessage = {
            id: "c",
            role: "assistant",
            parts: [{ type: "toolCall", id: "tc1", name: "noop", arguments: {} }],
            ordinal: 1,
        };
        const result: RawMessage = {
            id: "r",
            role: "user",
            parts: [{ role: "toolResult", toolCallId: "tc1", toolName: "noop" }],
            ordinal: 2,
        };
        expect(buildToolArcs([call, result], "pi-folded-v1")).toEqual([
            { callId: "tc1", invOrdinal: 1, resOrdinal: 2 },
        ]);
    });

    it("prefers a Pi toolCall's native id over a retained callId", () => {
        const call: RawMessage = {
            id: "c",
            role: "assistant",
            parts: [
                { type: "toolCall", id: "native-1", callId: "stale-9", name: "x", arguments: {} },
            ],
            ordinal: 1,
        };
        const result: RawMessage = {
            id: "r",
            role: "user",
            parts: [{ role: "toolResult", toolCallId: "native-1", content: "ok" }],
            ordinal: 2,
        };
        expect(buildToolArcs([call, result], "pi-folded-v1")).toEqual([
            { callId: "native-1", invOrdinal: 1, resOrdinal: 2 },
        ]);
    });

    it("closes a completed OpenCode tool that stored no input", () => {
        const message: RawMessage = {
            id: "no-input",
            role: "assistant",
            parts: [
                {
                    type: "tool",
                    callID: "c1",
                    tool: "bash",
                    state: { status: "completed", output: "done" },
                },
            ],
            ordinal: 1,
        };
        expect(buildToolArcs([message], "opencode-v1")).toEqual([
            { callId: "c1", invOrdinal: 1, resOrdinal: 1 },
        ]);
    });

    it("keeps a running OpenCode tool open even when partial output is stored", () => {
        const message: RawMessage = {
            id: "streaming",
            role: "assistant",
            parts: [
                {
                    type: "tool",
                    callID: "c",
                    tool: "bash",
                    state: { status: "running", input: {}, output: "partial stream..." },
                },
            ],
            ordinal: 1,
        };
        expect(buildToolArcs([message], "opencode-v1")).toEqual([
            { callId: "c", invOrdinal: 1, resOrdinal: null },
        ]);
    });

    it("does not treat an OpenCode-only tool type as a tool under the Pi shape", () => {
        const message: RawMessage = {
            id: "pi-opaque-tool",
            role: "assistant",
            parts: [{ type: "tool", payload: "X".repeat(8000) }],
            ordinal: 1,
        };
        expect(buildToolArcs([message], "pi-folded-v1")).toEqual([]);
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "pi-folded-v1",
        });
        expect(breakdown.other).toBeGreaterThan(100);
        expect(breakdown.toolInput).toBe(0);
    });
});

describe("tool token accounting", () => {
    it("counts an empty reasoning block as zero tokens", () => {
        const message = (part: unknown): RawMessage => ({
            id: "r",
            role: "assistant",
            parts: [part],
            ordinal: 1,
        });
        const options = { providerShapeVersion: "opencode-v1" as const };
        expect(
            estimateTrueRawMessageTokens(
                message({ type: "reasoning", text: "", signature: "x".repeat(200) }),
                options,
            ).total,
        ).toBe(0);
        expect(
            estimateTrueRawMessageTokens(message({ type: "thinking", thinking: "" }), options)
                .total,
        ).toBe(0);
        expect(
            estimateTrueRawMessageTokens(message({ type: "thinking", thinking: "hmm" }), options)
                .reasoning,
        ).toBeGreaterThan(0);
    });

    it("counts a redacted reasoning payload as reasoning and fingerprints it", () => {
        const message = (signature: string): RawMessage => ({
            id: "r",
            role: "assistant",
            parts: [
                { type: "thinking", thinking: "", thinkingSignature: signature, redacted: true },
            ],
            ordinal: 1,
        });
        const options = { providerShapeVersion: "pi-folded-v1" as const };
        expect(
            estimateTrueRawMessageTokens(message("R".repeat(2000)), options).reasoning,
        ).toBeGreaterThan(100);
        expect(
            computeRawRangeFingerprint([message("A".repeat(100))], 1, 2, "pi-folded-v1"),
        ).not.toBe(computeRawRangeFingerprint([message("B".repeat(3000))], 1, 2, "pi-folded-v1"));
        expect(
            estimateTrueRawMessageTokens(
                {
                    id: "anthropic",
                    role: "assistant",
                    parts: [{ type: "redacted_thinking", data: "D".repeat(1000) }],
                    ordinal: 1,
                },
                { providerShapeVersion: "opencode-v1" },
            ).reasoning,
        ).toBeGreaterThan(100);
    });

    it("skips OpenCode bookkeeping parts", () => {
        const message: RawMessage = {
            id: "bookkeeping",
            role: "assistant",
            parts: [
                { type: "snapshot", snapshot: "x".repeat(5000) },
                { type: "patch", hash: "abc", files: ["a", "b", "c"] },
                { type: "agent", name: "coder" },
                { type: "retry", attempt: 3 },
            ],
            ordinal: 1,
        };
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" }).total,
        ).toBe(0);
    });

    it("keeps Pi blocks that share OpenCode bookkeeping names as opaque content", () => {
        const message = singlePartMessage({ type: "snapshot", payload: "S".repeat(8000) });
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "pi-folded-v1" }).other,
        ).toBeGreaterThan(1000);
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" }).total,
        ).toBe(0);
    });

    it("never reads content as text for a text part", () => {
        const message = singlePartMessage({ type: "text", content: "stale content ".repeat(100) });
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" }).total,
        ).toBe(0);
    });

    it("ignores non-string OpenCode output values", () => {
        const message = singlePartMessage({
            type: "tool",
            callID: "c",
            tool: "x",
            state: {
                status: "completed",
                input: {},
                output: { rows: Array.from({ length: 500 }, (_, i) => ({ i, v: "vvvv" })) },
            },
        });
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" })
                .toolOutput,
        ).toBe(0);
    });

    it("does not fall back to top-level attachments when state.attachments is present", () => {
        const message = singlePartMessage({
            type: "tool",
            callID: "c",
            tool: "x",
            attachments: [{ mime: "image/png", url: "u" }],
            state: { status: "completed", input: {}, output: "", attachments: "not-an-array" },
        });
        expect(
            estimateTrueRawMessageTokens(message, {
                providerShapeVersion: "opencode-v1",
                imageTokenHeuristic: () => 400,
            }).image,
        ).toBe(0);
    });

    it("skips ignored OpenCode text and fingerprints the flag", () => {
        const text = "routing notice ".repeat(50);
        const options = { providerShapeVersion: "opencode-v1" as const };
        const visible = singlePartMessage({ type: "text", text });
        const ignored = singlePartMessage({ type: "text", text, ignored: true });
        expect(estimateTrueRawMessageTokens(visible, options).total).toBeGreaterThan(0);
        expect(estimateTrueRawMessageTokens(ignored, options).total).toBe(0);
        expect(fingerprintOf({ type: "text", text: "x" })).not.toBe(
            fingerprintOf({ type: "text", text: "x", ignored: true }),
        );
    });

    it("counts OpenCode redacted reasoning stored in metadata or a string redacted field", () => {
        const options = { providerShapeVersion: "opencode-v1" as const };
        const viaMetadata = singlePartMessage({
            type: "reasoning",
            text: "",
            metadata: { redacted: "R".repeat(2000) },
        });
        const viaField = singlePartMessage({
            type: "reasoning",
            text: "",
            redacted: "S".repeat(2000),
        });
        expect(estimateTrueRawMessageTokens(viaMetadata, options).reasoning).toBeGreaterThan(100);
        expect(estimateTrueRawMessageTokens(viaField, options).reasoning).toBeGreaterThan(100);
        expect(
            fingerprintOf({ type: "reasoning", text: "", metadata: { redacted: "A".repeat(100) } }),
        ).not.toBe(
            fingerprintOf({
                type: "reasoning",
                text: "",
                metadata: { redacted: "B".repeat(3000) },
            }),
        );
    });

    it("counts a standalone file part through the media heuristic", () => {
        const pdf = singlePartMessage({
            type: "file",
            mime: "application/pdf",
            filename: "spec.pdf",
            url: `data:application/pdf;base64,${"Q".repeat(30_000)}`,
        });
        const breakdown = estimateTrueRawMessageTokens(pdf, {
            providerShapeVersion: "opencode-v1",
            imageTokenHeuristic: () => 400,
        });
        expect(breakdown.image).toBe(400);
        expect(breakdown.other).toBe(0);
        expect(breakdown.text).toBe(0);
    });

    it("keeps a typed text block as text even when it carries MIME metadata", () => {
        const message = singlePartMessage({
            type: "tool_result",
            tool_use_id: "c",
            content: [{ type: "text", text: "the actual output text here", mimeType: null }],
        });
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "pi-folded-v1",
        });
        expect(breakdown.toolOutput).toBeGreaterThan(0);
        expect(breakdown.image).toBe(0);
    });

    it("prefers the type-native reasoning field over a stale sibling", () => {
        const options = { providerShapeVersion: "opencode-v1" as const };
        const openCode = estimateTrueRawMessageTokens(
            singlePartMessage({
                type: "reasoning",
                text: "new",
                thinking: "old very long stale thinking text ".repeat(20),
            }),
            options,
        );
        const pi = estimateTrueRawMessageTokens(
            singlePartMessage({
                type: "thinking",
                thinking: "new",
                text: "old very long stale text ".repeat(20),
            }),
            options,
        );
        expect(openCode.reasoning).toBeLessThan(10);
        expect(pi.reasoning).toBeLessThan(10);
    });

    it("lets an explicitly empty OpenCode text field win over retained content", () => {
        const message = singlePartMessage({
            type: "text",
            text: "",
            content: "stale compat content ".repeat(30),
        });
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" }).total,
        ).toBe(0);
    });

    it("skips compaction marker parts", () => {
        const message = singlePartMessage({
            type: "compaction",
            summary: "x".repeat(3000),
            metadata: { a: 1 },
        });
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" }).total,
        ).toBe(0);
    });

    it("counts step boundary parts like the daemon's boundary index", () => {
        const message: RawMessage = {
            id: "steps",
            role: "assistant",
            parts: [
                { type: "step-start", snapshot: "abc123" },
                {
                    type: "step-finish",
                    reason: "stop",
                    cost: 0.01,
                    tokens: { input: 100, output: 50 },
                },
            ],
            ordinal: 1,
        };
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" }).other,
        ).toBeGreaterThan(0);
    });

    it("treats every standalone file part as media, even with inline content", () => {
        const message = singlePartMessage({
            type: "file",
            mime: "text/plain",
            filename: "big.txt",
            content: "lorem ipsum ".repeat(2000),
        });
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
            imageTokenHeuristic: () => 400,
        });
        expect(breakdown.image).toBe(400);
        expect(breakdown.text).toBe(0);
    });

    it("keeps a typed non-media result block opaque even when it carries a MIME field", () => {
        const message = singlePartMessage({
            type: "tool_result",
            tool_use_id: "c",
            content: [{ type: "opaque", mimeType: "application/x-custom", data: "Z".repeat(8000) }],
        });
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "pi-folded-v1",
        });
        expect(breakdown.toolOutput).toBeGreaterThan(1000);
        expect(breakdown.image).toBe(0);
    });

    it("serializes a typed opaque result block whole even when it carries a text field", () => {
        const block = (data: string) => ({
            type: "tool_result",
            tool_use_id: "c",
            content: [{ type: "opaque", text: "ok", data }],
        });
        const breakdown = estimateTrueRawMessageTokens(
            singlePartMessage(block("Q".repeat(10_000))),
            {
                providerShapeVersion: "pi-folded-v1",
            },
        );
        expect(breakdown.toolOutput).toBeGreaterThan(1000);
        expect(fingerprintOf(block("A".repeat(100)))).not.toBe(
            fingerprintOf(block("B".repeat(5000))),
        );
    });

    it("treats a typed OpenCode attachment with a MIME field as media", () => {
        const message = singlePartMessage({
            type: "tool",
            callID: "c",
            tool: "read",
            state: {
                status: "completed",
                input: {},
                output: "",
                attachments: [
                    { type: "attachment", mime: "application/pdf", data: "P".repeat(20_000) },
                ],
            },
        });
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
            imageTokenHeuristic: () => 400,
        });
        expect(breakdown.image).toBe(400);
        expect(breakdown.toolOutput).toBe(0);
    });

    it("ignores a result field on OpenCode tool parts", () => {
        const message = singlePartMessage({
            type: "tool",
            callID: "c",
            tool: "x",
            state: { status: "completed", input: {}, result: "R".repeat(10_000) },
        });
        expect(
            estimateTrueRawMessageTokens(message, { providerShapeVersion: "opencode-v1" })
                .toolOutput,
        ).toBe(0);
    });

    it("treats a text-typed OpenCode attachment with a MIME field as media", () => {
        const message = singlePartMessage({
            type: "tool",
            callID: "c",
            tool: "read",
            state: {
                status: "completed",
                input: {},
                output: "",
                attachments: [{ type: "text", mime: "application/pdf", data: "P".repeat(20_000) }],
            },
        });
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
            imageTokenHeuristic: () => 400,
        });
        expect(breakdown.image).toBe(400);
        expect(breakdown.toolOutput).toBe(0);
    });

    it("classifies only an exact file type as media", () => {
        const breakdown = estimateTrueRawMessageTokens(
            singlePartMessage({ type: "profile", payload: "X".repeat(20_000) }),
            { providerShapeVersion: "opencode-v1" },
        );
        expect(breakdown.image).toBe(0);
        expect(breakdown.other).toBeGreaterThan(1000);
    });

    it("keeps source parts opaque", () => {
        const part = (payload: string) => ({ type: "source", content: "short", payload });
        const breakdown = estimateTrueRawMessageTokens(
            singlePartMessage(part("Y".repeat(20_000))),
            {
                providerShapeVersion: "opencode-v1",
            },
        );
        expect(breakdown.text).toBe(0);
        expect(breakdown.other).toBeGreaterThan(1000);
        expect(fingerprintOf(part("A".repeat(10)))).not.toBe(fingerprintOf(part("B".repeat(5000))));
    });

    it("counts an empty text block in a tool result as empty output", () => {
        const result = (text: string): RawMessage => ({
            id: "r",
            role: "user",
            parts: [{ type: "tool_result", tool_use_id: "c", content: [{ type: "text", text }] }],
            ordinal: 1,
        });
        const empty = estimateTrueRawMessageTokens(result(""), {
            providerShapeVersion: "opencode-v1",
        });
        const short = estimateTrueRawMessageTokens(result("ok"), {
            providerShapeVersion: "opencode-v1",
        });
        expect(empty.toolOutput).toBe(0);
        expect(short.toolOutput).toBeGreaterThan(0);
    });

    it("counts OpenCode tool attachments as media", () => {
        const message: RawMessage = {
            id: "read",
            role: "assistant",
            parts: [
                {
                    type: "tool",
                    callID: "c1",
                    tool: "read",
                    state: {
                        status: "completed",
                        input: { path: "x.png" },
                        output: "",
                        attachments: [{ type: "file", mime: "image/png", url: "file:///x.png" }],
                    },
                },
            ],
            ordinal: 1,
        };
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
            imageTokenHeuristic: () => 333,
        });
        expect(breakdown.image).toBe(333);
    });

    it("counts media blocks in tool results through the image heuristic, not as text", () => {
        const message: RawMessage = {
            id: "result",
            role: "user",
            parts: [
                {
                    type: "tool_result",
                    tool_use_id: "c1",
                    content: [
                        { type: "text", text: "done" },
                        { type: "image", mimeType: "image/png", data: "A".repeat(20_000) },
                        { type: "file", mimeType: "application/pdf", data: "B".repeat(20_000) },
                    ],
                },
            ],
            ordinal: 1,
        };
        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
            imageTokenHeuristic: () => 300,
        });
        expect(breakdown.toolOutput).toBeLessThan(10);
        expect(breakdown.image).toBe(600);
    });

    it("counts every input when several parts share one call id", () => {
        const part = (cmd: string) => ({
            type: "tool_use",
            id: "dup",
            name: "bash",
            input: { cmd },
        });
        const single = estimateTrueRawMessageTokens(
            { id: "one", role: "assistant", parts: [part("first command here")], ordinal: 1 },
            { providerShapeVersion: "opencode-v1" },
        );
        const double = estimateTrueRawMessageTokens(
            {
                id: "two",
                role: "assistant",
                parts: [part("first command here"), part("second command here")],
                ordinal: 1,
            },
            { providerShapeVersion: "opencode-v1" },
        );
        expect(double.toolInput).toBeGreaterThan(single.toolInput);
    });
});

describe("tool arc fences", () => {
    it("retreats to the invocation when a completed arc above the floor straddles the candidate", () => {
        const arcs = [{ callId: "c", invOrdinal: 3, resOrdinal: 7 }];
        expect(fenceBoundaryForToolArcs(5, arcs, 1, 1)).toBe(3);
    });

    it("advances past the result when the invocation is already below the floor", () => {
        const arcs = [{ callId: "c", invOrdinal: 1, resOrdinal: 3 }];
        expect(fenceBoundaryForToolArcs(2, arcs, 1, 1)).toBe(4);
    });

    it("fences the whole overlapping component so no completed arc is split", () => {
        const arcs = [
            { callId: "ordinary", invOrdinal: 122, resOrdinal: 124 },
            { callId: "reasoning", invOrdinal: 123, resOrdinal: 125 },
        ];
        const fenced = fenceBoundaryForToolArcs(125, arcs, 1, 1);
        expect(fenced).toBe(122);
        for (const arc of arcs) {
            expect(completedToolArcCrossesBoundary(arc.invOrdinal, arc.resOrdinal, fenced)).toBe(
                false,
            );
        }
    });

    it("leaves a candidate untouched when no completed arc straddles it", () => {
        const arcs = [{ callId: "c", invOrdinal: 2, resOrdinal: 4 }];
        expect(fenceBoundaryForToolArcs(5, arcs, 1, 1)).toBe(5);
        expect(fenceBoundaryForToolArcs(2, arcs, 1, 1)).toBe(2);
    });

    it("re-fences completed arcs after an open arc pulls the boundary back", () => {
        const arcs = [
            { callId: "done", invOrdinal: 4, resOrdinal: 8 },
            { callId: "open", invOrdinal: 6, resOrdinal: null },
        ];
        expect(fenceBoundaryForToolArcs(10, arcs, 1, 1)).toBe(4);
    });
});

describe("message token cache keys", () => {
    it("invalidates by session id even when the namespace does not contain it", () => {
        const message = (text: string): RawMessage => ({
            id: "msg",
            role: "user",
            parts: [{ type: "text", text, updated_at: 1 }],
            ordinal: 1,
        });
        const options = {
            providerShapeVersion: "opencode-v1" as const,
            cacheNamespace: "unrelated-namespace",
        };

        const ascii = buildTrueRawTokenIndex("session-A", [message("hello world foo")], options);
        expect(ascii.tokenForOrdinal(1)).toBeGreaterThan(0);

        invalidateTrueRawTokenCache({ sessionId: "session-A", reason: "message.updated" });

        const cjk = buildTrueRawTokenIndex(
            "session-A",
            [message("日本語日本語日本語日本語日本語")],
            options,
        );
        const fresh = buildTrueRawTokenIndex(
            "session-A",
            [message("日本語日本語日本語日本語日本語")],
            { ...options, cacheNamespace: "unrelated-namespace-fresh" },
        );
        expect(cjk.tokenForOrdinal(1)).toBe(fresh.tokenForOrdinal(1));
        expect(cjk.tokenForOrdinal(1)).not.toBe(ascii.tokenForOrdinal(1));
    });

    it("does not invalidate a session whose id merely contains the target id", () => {
        const message = (text: string): RawMessage => ({
            id: "msg",
            role: "user",
            parts: [{ type: "text", text, updated_at: 1 }],
            ordinal: 1,
        });
        const options = {
            providerShapeVersion: "opencode-v1" as const,
            cacheNamespace: "prefix-namespace",
        };

        const ascii = buildTrueRawTokenIndex("s-1", [message("hello world foo")], options);
        invalidateTrueRawTokenCache({ sessionId: "s", reason: "session.deleted" });
        const stillCached = buildTrueRawTokenIndex(
            "s-1",
            [message("日本語日本語日本語日本語日本語")],
            options,
        );
        expect(stillCached.tokenForOrdinal(1)).toBe(ascii.tokenForOrdinal(1));
    });

    it("separates cache entries by image heuristic identity", () => {
        const message = (): RawMessage => ({
            id: "image-message",
            role: "user",
            parts: [{ type: "image", mime: "image/png", width: 100, height: 100, updated_at: 7 }],
            ordinal: 1,
        });
        const options = {
            providerShapeVersion: "opencode-v1" as const,
            cacheNamespace: "image-heuristic-test",
        };
        const eleven = () => 11;
        const many = () => 999;

        expect(
            buildTrueRawTokenIndex("s", [message()], {
                ...options,
                imageTokenHeuristic: eleven,
            }).tokenForOrdinal(1),
        ).toBe(11);
        expect(
            buildTrueRawTokenIndex("s", [message()], {
                ...options,
                imageTokenHeuristic: many,
            }).tokenForOrdinal(1),
        ).toBe(999);
        expect(buildTrueRawTokenIndex("s", [message()], options).tokenForOrdinal(1)).toBe(256);
        expect(
            buildTrueRawTokenIndex("s", [message()], {
                ...options,
                imageTokenHeuristic: eleven,
            }).tokenForOrdinal(1),
        ).toBe(11);
    });

    it("does not reuse a cached breakdown for unversioned parts of equal length but different content", () => {
        const options = {
            providerShapeVersion: "opencode-v1" as const,
            cacheNamespace: "unversioned-parts-test",
        };
        const imagePart = (side: number) => ({
            id: "same-message",
            role: "user",
            parts: [{ type: "image", mime: "image/png", width: side, height: side }],
            ordinal: 1,
            version: null,
        });

        const small = buildTrueRawTokenIndex("s", [imagePart(100)], options).tokenForOrdinal(1);
        const large = buildTrueRawTokenIndex("s", [imagePart(900)], options).tokenForOrdinal(1);
        const largeFresh = buildTrueRawTokenIndex("s", [imagePart(900)], {
            ...options,
            cacheNamespace: "unversioned-parts-fresh-test",
        }).tokenForOrdinal(1);

        expect(large).toBe(largeFresh);
        expect(large).not.toBe(small);
    });
});

describe("raw range fingerprints", () => {
    it("changes when any tokenized non-tool field changes", () => {
        expect(fingerprintOf({ type: "source", source: "alpha" })).not.toBe(
            fingerprintOf({ type: "source", source: "omega-different" }),
        );
        expect(
            fingerprintOf({ type: "image", mime: "image/png", width: 100, height: 100 }),
        ).not.toBe(fingerprintOf({ type: "image", mime: "image/png", width: 2000, height: 2000 }));
        expect(fingerprintOf({ type: "image", mime: "image/png", alt: "a cat" })).not.toBe(
            fingerprintOf({ type: "image", mime: "image/png", alt: "a dog on a long walk" }),
        );
        expect(fingerprintOf({ type: "file", content: "one" })).not.toBe(
            fingerprintOf({ type: "file", content: "two" }),
        );
    });

    it("hashes the whole part when the tokenizer falls back to serializing it", () => {
        expect(fingerprintOf({ type: "file", url: "file:///a.txt", mime: "text/plain" })).not.toBe(
            fingerprintOf({ type: "file", url: "file:///b-longer-name.txt", mime: "text/plain" }),
        );
        expect(fingerprintOf({ type: "custom", payload: { a: 1 } })).not.toBe(
            fingerprintOf({ type: "custom", payload: { a: 2 } }),
        );
    });

    it("changes when a tool part's identity or completion state changes", () => {
        const pending = { type: "tool-invocation", toolCallId: "c1", toolName: "bash", args: {} };
        expect(fingerprintOf(pending)).not.toBe(fingerprintOf({ ...pending, result: null }));
        expect(fingerprintOf(pending)).not.toBe(fingerprintOf({ ...pending, toolCallId: "c2" }));
        expect(fingerprintOf(pending)).not.toBe(fingerprintOf({ ...pending, toolName: "grep" }));
        expect(fingerprintOf({ type: "tool", callID: "c1", state: { input: {} } })).not.toBe(
            fingerprintOf({
                type: "tool",
                callID: "c1",
                metadata: { providerExecuted: true },
                state: { input: {} },
            }),
        );
    });

    it("changes when a tool's metadata description or attachments change", () => {
        const base = {
            type: "tool",
            callID: "c1",
            tool: "task",
            state: { status: "completed", input: { x: 1 }, output: "ok" },
        };
        expect(
            fingerprintOf({
                ...base,
                state: { ...base.state, metadata: { description: "Explore repo" } },
            }),
        ).not.toBe(
            fingerprintOf({
                ...base,
                state: { ...base.state, metadata: { description: "Fix the bug" } },
            }),
        );
        expect(fingerprintOf(base)).not.toBe(
            fingerprintOf({
                ...base,
                state: {
                    ...base.state,
                    attachments: [{ type: "file", mime: "image/png", url: "x" }],
                },
            }),
        );
    });

    it("hashes every media field a custom image heuristic could read", () => {
        const image = { type: "image", mime: "image/png", width: 10, height: 10 };
        expect(fingerprintOf({ ...image, detail: "low", url: "u1" })).not.toBe(
            fingerprintOf({ ...image, detail: "high", url: "u2-longer" }),
        );
    });

    it("ignores updated-at metadata", () => {
        expect(fingerprintOf({ type: "text", text: "same", updated_at: 1 })).toBe(
            fingerprintOf({ type: "text", text: "same", updated_at: 2 }),
        );
    });

    it("changes when a message's role changes", () => {
        const message = (role: string): RawMessage[] => [
            { id: "x", role, parts: [{ type: "text", text: "hi" }], ordinal: 1 },
        ];
        expect(computeRawRangeFingerprint(message("user"), 1, 2, "opencode-v1")).not.toBe(
            computeRawRangeFingerprint(message("assistant"), 1, 2, "opencode-v1"),
        );
    });

    it("changes when a tool result flips between success and error", () => {
        const openCode = (status: string, key: string) => ({
            type: "tool",
            callID: "c",
            tool: "x",
            state: { status, input: {}, [key]: "same" },
        });
        expect(fingerprintOf(openCode("completed", "output"))).not.toBe(
            fingerprintOf(openCode("error", "error")),
        );
        const pi = (isError: boolean) => ({
            role: "toolResult",
            toolCallId: "c",
            content: "same",
            ...(isError ? { isError: true } : {}),
        });
        expect(fingerprintOf(pi(false), "pi-folded-v1")).not.toBe(
            fingerprintOf(pi(true), "pi-folded-v1"),
        );
    });

    it("hashes primitive and array parts by content", () => {
        expect(fingerprintOf("hello world!")).not.toBe(fingerprintOf("HELLO WORLD?"));
        expect(fingerprintOf([1, 2, 3])).not.toBe(fingerprintOf([4, 5, 6]));
        const options = {
            providerShapeVersion: "opencode-v1" as const,
            cacheNamespace: "primitive-parts-test",
        };
        const message = (part: unknown): RawMessage => ({
            id: "x",
            role: "user",
            parts: [part],
            ordinal: 1,
        });
        buildTrueRawTokenIndex("s", [message("hello world!")], options);
        const cjk = buildTrueRawTokenIndex("s", [message("日本語日本語日本語日本語")], options);
        const fresh = buildTrueRawTokenIndex("s", [message("日本語日本語日本語日本語")], {
            ...options,
            cacheNamespace: "primitive-parts-fresh-test",
        });
        expect(cjk.tokenForOrdinal(1)).toBe(fresh.tokenForOrdinal(1));
    });
});
