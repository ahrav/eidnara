import { describe, expect, it } from "bun:test";

import { extractEidnaraSearchQueryInput, normalizeEidnaraSearchArgs } from "./query-input";

describe("normalizeEidnaraSearchArgs", () => {
    it("passes ordinary args through unchanged", () => {
        const args = { query: "find the retry policy", limit: 5 };
        expect(normalizeEidnaraSearchArgs(args)).toEqual(args);
    });

    it("unwraps the imitated-reduced argument shape", () => {
        const args = {
            reduced: true,
            summary: JSON.stringify({ query: "nested lookup", sources: ["memory"] }),
        };
        expect(normalizeEidnaraSearchArgs(args)).toMatchObject({ query: "nested lookup" });
    });

    it("keeps malformed nested summaries as-is instead of guessing", () => {
        const args = { reduced: true, summary: "not json at all" };
        expect(normalizeEidnaraSearchArgs(args)).toEqual(args);
    });
});

describe("extractEidnaraSearchQueryInput", () => {
    it("trims and accepts an ordinary query", () => {
        expect(extractEidnaraSearchQueryInput({ query: "  spaced query  " })).toEqual({
            ok: true,
            query: "spaced query",
        });
    });

    it("returns the empty query for omitted input", () => {
        expect(extractEidnaraSearchQueryInput({})).toEqual({ ok: true, query: "" });
    });

    it("rejects an over-cap query with the byte violation", () => {
        const result = extractEidnaraSearchQueryInput({ query: "x".repeat(17 * 1024) });
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.violation).toBe("bytes");
    });
});
