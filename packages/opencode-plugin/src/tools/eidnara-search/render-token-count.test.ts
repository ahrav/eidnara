import { describe, expect, it } from "bun:test";
import { estimateTokens } from "../../shared/token-estimator";
import { MAX_RENDERED_RESULT_TOKENS } from "./bounds";
import type { MemorySearchResult } from "./kernel-memory-search";
import { packSearchResults } from "./render";

/** Endings that change how the tokenizer splits the text at a section boundary. */
const TAILS = [
    "",
    " ",
    "   ",
    "\n",
    "\r\n",
    "\t \n",
    "\u3000",
    "\u00a0\u2003",
    "\ufeff",
    "\u0085",
    "'s",
    "'ll",
    "'",
    "🎉",
    "é",
    " ".repeat(5000),
    "\n".repeat(3),
];
const WORDS = ["store", "retry", "日本語", "Ünïcödé", "3.14", "fsync()", "--locked", "x'd", "a"];

function result(index: number, content: string): MemorySearchResult {
    return {
        source: "memory",
        content,
        score: 1 / (index + 1),
        objectId: `mem_${String(index).padStart(32, "0")}`,
        category: "NAMING",
        matchType: "fused",
        lanes: ["lexical", "dense"],
    };
}

function contentFor(seed: number): string {
    const words = Array.from(
        { length: 20 + ((seed * 37) % 160) },
        (_, index) => WORDS[(seed + index * 7) % WORDS.length],
    );
    return `${words.join(" ")}${TAILS[seed % TAILS.length]}`;
}

/** Tokens of the header, the first `kept` blocks at their ranked positions, and the omission notice. */
function candidateTokens(query: string, results: MemorySearchResult[], kept: number): number {
    const single = `Found 1 result for "${query}":\n\n`;
    const blocks = results.slice(0, kept).map((entry, index) =>
        packSearchResults(query, [{ ...entry, score: 1 }])
            .text.slice(single.length)
            .replace(
                "[1] [memory] score=1.00",
                `[${index + 1}] [memory] score=${entry.score.toFixed(2)}`,
            ),
    );
    const omitted = results.length - kept;
    const notice = `(${omitted} result${omitted === 1 ? "" : "s"} omitted to fit the output budget — refine the query or lower the limit)`;
    const header = `Found ${results.length} results for "${query}":`;
    return estimateTokens([header, ...blocks, notice].join("\n\n"));
}

describe("packSearchResults token accounting", () => {
    it("reports the token count of the delivered text for every section boundary shape", () => {
        for (let seed = 0; seed < 120; seed += 1) {
            const count = 1 + (seed % 32);
            const results = Array.from({ length: count }, (_, index) =>
                result(index, contentFor(seed * 31 + index)),
            );
            const preamble =
                seed % 3 === 0 ? `Memory: note${TAILS[seed % TAILS.length]}` : undefined;
            const packed = packSearchResults(`query ${seed}`, results, preamble);
            expect(packed.tokenCount).toBe(estimateTokens(packed.text));
        }
    });

    it("counts text holding a lone surrogate as one string", () => {
        for (const count of [2, 12, 32]) {
            const results = Array.from({ length: count }, (_, index) =>
                result(index, `${"store retry ".repeat(40)}${index < count - 1 ? "\ud800" : ""}`),
            );
            const packed = packSearchResults("plain", results);
            expect(packed.tokenCount).toBe(estimateTokens(packed.text));
            expect(packed.tokenCount).toBeLessThanOrEqual(MAX_RENDERED_RESULT_TOKENS);
        }
    });

    it("delivers the largest prefix whose text fits the budget", () => {
        const query = "budget";
        const results = Array.from({ length: 32 }, (_, index) =>
            result(index, contentFor(index * 3 + 1)),
        );
        const packed = packSearchResults(query, results);
        const kept = packed.delivered.length;
        expect(kept).toBeGreaterThan(0);
        expect(kept).toBeLessThan(results.length);
        expect(candidateTokens(query, results, kept)).toBe(packed.tokenCount);
        expect(candidateTokens(query, results, kept + 1)).toBeGreaterThan(
            MAX_RENDERED_RESULT_TOKENS,
        );
    });

    it("delivers every result whose text is too long to count whole but fits the budget", () => {
        const words = "consumers handlers projection snapshot deadline executor ";
        const results = Array.from({ length: 14 }, (_, index) =>
            result(index, words.repeat(18).slice(0, 1000)),
        );
        const packed = packSearchResults("long words", results);
        expect(packed.text.length).toBeGreaterThan(3 * MAX_RENDERED_RESULT_TOKENS);
        expect(packed.delivered).toEqual(results);
        expect(packed.tokenCount).toBe(estimateTokens(packed.text));
        expect(packed.tokenCount).toBeLessThanOrEqual(MAX_RENDERED_RESULT_TOKENS);
    });

    it("falls back to the largest fitting prefix when short text holds more tokens than the budget", () => {
        const query = "dense";
        const results = Array.from({ length: 12 }, (_, index) =>
            result(index, "日本語のテキストと漢字".repeat(40)),
        );
        const contentUnits = results.reduce((units, entry) => units + entry.content.length, 0);
        expect(contentUnits).toBeLessThan(2 * MAX_RENDERED_RESULT_TOKENS);
        const packed = packSearchResults(query, results);
        const kept = packed.delivered.length;
        expect(kept).toBeGreaterThan(0);
        expect(kept).toBeLessThan(results.length);
        expect(packed.tokenCount).toBe(estimateTokens(packed.text));
        expect(candidateTokens(query, results, kept)).toBe(packed.tokenCount);
        expect(candidateTokens(query, results, kept + 1)).toBeGreaterThan(
            MAX_RENDERED_RESULT_TOKENS,
        );
    });
});
