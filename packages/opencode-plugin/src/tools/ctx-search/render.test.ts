import { describe, expect, it } from "bun:test";
import { estimateTokens } from "../../shared/token-estimator";
import { boundDynamicField, MAX_RENDER_FIELD_BYTES, MAX_RENDERED_RESULT_TOKENS } from "./bounds";
import type {
    AntiMemorySearchResult,
    KernelMemorySearchResult,
    MemorySearchResult,
} from "./kernel-memory-search";
import { packSearchResults } from "./render";

/** formatSearchResults keeps rendering contract tests focused on emitted text.
 * */
function formatSearchResults(query: string, results: KernelMemorySearchResult[]): string {
    return packSearchResults(query, results).text;
}

function memoryResult(id: number, content: string): MemorySearchResult {
    return {
        source: "memory",
        content,
        score: 0.9,
        objectId: `mem_${String(id).padStart(32, "0")}`,
        category: "decision",
        matchType: "exact",
    };
}

function antiMemoryResult(rationale?: string): AntiMemorySearchResult {
    return {
        source: "anti_memory",
        score: 0.5,
        objectId: `mem_${"c".repeat(32)}`,
        contentDigest: "d".repeat(64),
        normalizedHash: "d".repeat(64),
        trigger: "session caching",
        rejectedStrategy: "Redis",
        rejectionReason: "it creates split ownership",
        saferAlternative: null,
        matchType: "lexical",
        ...(rationale ? { rationale } : {}),
    };
}

describe("boundDynamicField", () => {
    it("returns short fields unchanged", () => {
        expect(boundDynamicField("short")).toBe("short");
    });

    it("cuts oversized fields to the byte cap on a code-point boundary", () => {
        const bounded = boundDynamicField("🎉".repeat(1000));
        expect(Buffer.byteLength(bounded, "utf8")).toBeLessThanOrEqual(MAX_RENDER_FIELD_BYTES);
        expect(Buffer.from(bounded, "utf8").toString("utf8")).toBe(bounded);
    });
});

describe("packed search text rendering", () => {
    it("freezes the under-budget shape", () => {
        const results: KernelMemorySearchResult[] = [
            memoryResult(7, "always use bd for tracking"),
            antiMemoryResult(),
        ];
        const text = formatSearchResults("queue", results);
        expect(text).toBe(
            [
                'Found 2 results for "queue":',
                "",
                `[1] [memory] score=0.90 id=mem_${"7".padStart(32, "0")} category=decision match=exact`,
                "always use bd for tracking",
                "",
                `[2] [anti-memory warning] score=0.50 id=mem_${"c".repeat(32)} match=lexical status=active`,
                `⚠ Previously rejected: Redis. Reason: it creates split ownership. Verify before proceeding: confirm the rejection no longer applies to session caching. (see mem_${"c".repeat(32)})`,
            ].join("\n"),
        );
    });

    it("keeps the empty-result message unchanged", () => {
        expect(formatSearchResults("nothing", [])).toBe(
            'No results found for "nothing" in project memories.',
        );
    });

    it("renders an anti-memory rationale after the warning line and omits it when absent", () => {
        const withRationale = formatSearchResults("nonce", [
            antiMemoryResult("the nonce handshake stalls under load"),
        ]);
        expect(withRationale).toContain("⚠ Previously rejected: Redis.");
        expect(withRationale).toContain("Rationale: the nonce handshake stalls under load");

        const withoutRationale = formatSearchResults("nonce", [antiMemoryResult()]);
        expect(withoutRationale).not.toContain("Rationale:");
    });

    it("bounds every dynamic field before tokenization", () => {
        const huge = "content ".repeat(4000);
        const text = formatSearchResults("q", [memoryResult(1, huge)]);
        const body = text.split("\n")[3] ?? "";
        expect(Buffer.byteLength(body, "utf8")).toBeLessThanOrEqual(MAX_RENDER_FIELD_BYTES);
    });

    it("bounds the echoed query", () => {
        const text = formatSearchResults("q".repeat(5000), []);
        expect(Buffer.byteLength(text, "utf8")).toBeLessThan(MAX_RENDER_FIELD_BYTES + 200);
    });

    it("keeps a ranked prefix of complete blocks under the token budget", () => {
        const filler = Array.from({ length: 300 }, (_, index) =>
            ((index * 2654435761) % 36).toString(36),
        ).join(" ");
        const results = Array.from({ length: 50 }, (_, index) =>
            memoryResult(index + 1, `${filler} tail-${index}`),
        );
        const text = formatSearchResults("big", results);
        expect(estimateTokens(text)).toBeLessThanOrEqual(MAX_RENDERED_RESULT_TOKENS);

        const shownBlocks = (text.match(/\[\d+\] \[memory\]/g) ?? []).length;
        expect(shownBlocks).toBeGreaterThan(0);
        expect(shownBlocks).toBeLessThan(50);
        // Shown blocks use consecutive indexes from 1 through shownBlocks.
        for (let index = 1; index <= shownBlocks; index += 1) {
            expect(text).toContain(`[${index}] [memory]`);
        }
        expect(text).toContain(
            `(${50 - shownBlocks} results omitted to fit the output budget — refine the query or lower the limit)`,
        );
        // Every shown block carries its tail marker.
        for (let index = 0; index < shownBlocks; index += 1) {
            expect(text).toContain(`tail-${index}`);
        }
    });

    it("omits a whole block rather than splitting it at the budget edge", () => {
        const filler = Array.from({ length: 300 }, (_, index) =>
            ((index * 48271) % 36).toString(36),
        ).join(" ");
        const results = Array.from({ length: 50 }, (_, index) =>
            memoryResult(index + 1, `${filler} sentinel-${index}`),
        );
        const text = formatSearchResults("edge", results);
        // A block is either fully present (header + its sentinel) or fully absent.
        const shownBlocks = (text.match(/\[\d+\] \[memory\]/g) ?? []).length;
        for (let index = 0; index < 50; index += 1) {
            const hasHeader = text.includes(`[${index + 1}] [memory]`);
            const hasBody = text.includes(`sentinel-${index}`);
            expect(hasHeader).toBe(hasBody);
            expect(hasHeader).toBe(index < shownBlocks);
        }
    });
});

describe("packSearchResults", () => {
    it("packs under-budget results with full delivery accounting", () => {
        const results: KernelMemorySearchResult[] = [
            memoryResult(7, "always use bd for tracking"),
            antiMemoryResult(),
        ];
        const packed = packSearchResults("queue", results);
        expect(packed.delivered).toEqual(results);
        expect(packed.omittedCount).toBe(0);
        expect(packed.reason).toBe("delivered");
        expect(packed.tokenCount).toBe(estimateTokens(packed.text));
    });

    it("reports empty results with the empty reason", () => {
        const packed = packSearchResults("nothing", []);
        expect(packed.delivered).toEqual([]);
        expect(packed.omittedCount).toBe(0);
        expect(packed.reason).toBe("empty-results");
    });

    it("delivers exactly the results whose complete blocks were rendered when over budget", () => {
        const filler = Array.from({ length: 300 }, (_, index) =>
            ((index * 2654435761) % 36).toString(36),
        ).join(" ");
        const results = Array.from({ length: 50 }, (_, index) =>
            memoryResult(index + 1, `${filler} tail-${index}`),
        );
        const packed = packSearchResults("big", results);
        expect(packed.reason).toBe("delivered");
        expect(packed.tokenCount).toBe(estimateTokens(packed.text));
        expect(packed.tokenCount).toBeLessThanOrEqual(MAX_RENDERED_RESULT_TOKENS);

        const shownBlocks = (packed.text.match(/\[\d+\] \[memory\]/g) ?? []).length;
        expect(packed.delivered.length).toBe(shownBlocks);
        expect(packed.delivered).toEqual(results.slice(0, shownBlocks));
        expect(packed.omittedCount).toBe(50 - shownBlocks);
        for (const delivered of results.slice(0, shownBlocks)) {
            expect(packed.text).toContain(`id=${delivered.objectId}`);
        }
        for (const [index, omitted] of results.slice(shownBlocks).entries()) {
            void omitted;
            expect(packed.text).not.toContain(`tail-${shownBlocks + index}`);
        }
    });

    it("counts a preamble against the budget so a near-limit result set stays under it", () => {
        const filler = Array.from({ length: 300 }, (_, index) =>
            ((index * 2654435761) % 36).toString(36),
        ).join(" ");
        const results = Array.from({ length: 50 }, (_, index) =>
            memoryResult(index + 1, `${filler} tail-${index}`),
        );
        const bare = packSearchResults("big", results);
        const preamble = `Memory: unresolved object ids (the daemon read stayed truncated): ${Array.from(
            { length: 64 },
            (_, index) => `mem_${String(index).padStart(32, "0")}`,
        ).join(", ")}`;
        const packed = packSearchResults("big", results, preamble);

        expect(packed.text).toStartWith(`${preamble}\n\nFound 50 results`);
        expect(packed.tokenCount).toBe(estimateTokens(packed.text));
        expect(packed.tokenCount).toBeLessThanOrEqual(MAX_RENDERED_RESULT_TOKENS);
        expect(estimateTokens(`${preamble}\n\n${bare.text}`)).toBeGreaterThan(
            MAX_RENDERED_RESULT_TOKENS,
        );
        expect(packed.delivered.length).toBeLessThan(bare.delivered.length);
        expect(packed.delivered).toEqual(results.slice(0, packed.delivered.length));
    });

    it("renders a preamble ahead of the empty-results line and counts it", () => {
        const packed = packSearchResults("nothing", [], "Memory: the memory read was truncated.");
        expect(packed.text).toBe(
            'Memory: the memory read was truncated.\n\nNo results found for "nothing" in project memories.',
        );
        expect(packed.tokenCount).toBe(estimateTokens(packed.text));
        expect(packed.reason).toBe("empty-results");
    });
});
