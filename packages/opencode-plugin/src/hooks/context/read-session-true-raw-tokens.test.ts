/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";

import type { RawMessage } from "./read-session-raw";
import {
    buildToolArcs,
    buildTrueRawTokenIndex,
    buildTrueRawTokenIndexFromTokenCountsForTest,
    computeRawRangeFingerprint,
    estimateTrueRawMessageTokens,
} from "./read-session-true-raw-tokens";

function singlePartMessage(part: unknown, ordinal = 1): RawMessage {
    return { id: `m-${ordinal}`, role: "user", parts: [part], ordinal };
}

function fingerprintOf(part: unknown): string {
    return computeRawRangeFingerprint([singlePartMessage(part)], 1, 2);
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

        expect(buildToolArcs(messages)).toEqual([
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

        expect(buildToolArcs([message])).toEqual([]);

        const breakdown = estimateTrueRawMessageTokens(message, {
            providerShapeVersion: "opencode-v1",
        });
        expect(breakdown.toolInput).toBeGreaterThan(0);
    });
});

describe("message token cache keys", () => {
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

    it("ignores updated-at metadata", () => {
        expect(fingerprintOf({ type: "text", text: "same", updated_at: 1 })).toBe(
            fingerprintOf({ type: "text", text: "same", updated_at: 2 }),
        );
    });
});
