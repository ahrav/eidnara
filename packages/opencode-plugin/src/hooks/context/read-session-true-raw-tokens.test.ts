/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";

import type { RawMessage } from "./read-session-raw";
import { buildTrueRawTokenIndex, markPartMutated } from "./read-session-true-raw-tokens";

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
});

describe("message estimate cache after in-place part mutation", () => {
    // The strings have equal byte length but different token counts.
    const prose = "the quick brown fox jumps over the lazy dog and runs away fast";
    const noise = "xq7z-k2p9 v4mn!8rt@ w1yb#5ju% e3ho&6ci* a0sd(2fg) h9lk_7pz+abc";

    function buildFor(part: { state: { output: string } }): number {
        const messages: RawMessage[] = [{ id: "m", role: "assistant", parts: [part], ordinal: 1 }];
        return buildTrueRawTokenIndex("mutation", messages, {
            providerShapeVersion: "opencode-v1",
            cacheNamespace: "mutation-test",
            absoluteMessageCount: 1,
        }).tokenForOrdinal(1);
    }

    it("reuses the cached count when the fingerprint is unchanged", () => {
        expect(prose.length).toBe(noise.length);
        const part = { type: "tool", callID: "c", state: { output: prose } };
        const before = buildFor(part);
        part.state.output = noise;
        expect(buildFor(part)).toBe(before);
    });

    it("recounts after markPartMutated advances the part version", () => {
        const part = { type: "tool", callID: "c", state: { output: prose } };
        const before = buildFor(part);
        part.state.output = noise;
        markPartMutated(part);
        const after = buildFor(part);
        expect(after).not.toBe(before);
        expect(JSON.stringify(part)).not.toContain("__eidnaraPartUpdatedAt");
    });
});
