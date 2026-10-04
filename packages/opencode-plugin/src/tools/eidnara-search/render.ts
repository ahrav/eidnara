/**
 * This module renders explicit `eidnara_search` output under a token budget.
 *
 * The renderer bounds each dynamic result field to `MAX_DYNAMIC_FIELD_BYTES` of valid UTF-8 before tokenization.
 * Packing retains a ranked prefix of complete result blocks under `MAX_RENDERED_RESULT_TOKENS`.
 */

import { estimateTokens } from "../../shared/token-estimator";
import {
    binarySearchLargestFit,
    boundDynamicField,
    isWellFormed,
    MAX_RENDERED_RESULT_TOKENS,
    renderAntiMemoryWarningLine,
} from "./bounds";
import type { AntiMemorySearchResult, KernelMemorySearchResult } from "./kernel-memory-search";

export function renderAntiMemoryWarning(result: AntiMemorySearchResult): string {
    return renderAntiMemoryWarningLine({
        trigger: result.trigger,
        rejectedStrategy: result.rejectedStrategy,
        rejectionReason: result.rejectionReason,
        saferAlternative: result.saferAlternative,
        boundField: boundDynamicField,
        citation: result.objectId,
    });
}

function lanesOf(result: KernelMemorySearchResult): string {
    return result.lanes && result.lanes.length > 0 ? ` lanes=${result.lanes.join(",")}` : "";
}

function formatResult(result: KernelMemorySearchResult, index: number): string {
    if (result.source === "anti_memory") {
        const policy = result.policyLabel
            ? ` status=${boundDynamicField(result.policyLabel)}`
            : " status=active";
        return [
            `[${index}] [anti-memory warning] score=${result.score.toFixed(2)} id=${result.objectId} match=${result.matchType}${lanesOf(result)}${policy}`,
            renderAntiMemoryWarning(result),
            // The rationale renders only in this full-result view; the compact auto-search hint keeps the warning-line field set.
            ...(result.rationale ? [`Rationale: ${boundDynamicField(result.rationale)}`] : []),
        ].join("\n");
    }
    const source = result.sourceName ? ` source=${boundDynamicField(result.sourceName)}` : "";
    const policy = result.policyLabel ? ` trust=[${boundDynamicField(result.policyLabel)}]` : "";
    return [
        `[${index}] [memory] score=${result.score.toFixed(2)} id=${result.objectId} category=${boundDynamicField(result.category)}${source} match=${result.matchType}${lanesOf(result)}${policy}`,
        boundDynamicField(result.content),
    ].join("\n");
}

const SECTION_SEPARATOR = "\n\n";

function assemble(sections: readonly string[]): string {
    return sections.join(SECTION_SEPARATOR);
}

/** The tokenizer's whitespace class: ECMAScript WhiteSpace and LineTerminator. */
function isTokenizerWhitespace(code: number): boolean {
    return (
        (code >= 0x09 && code <= 0x0d) ||
        code === 0x20 ||
        code === 0xa0 ||
        code === 0x1680 ||
        (code >= 0x2000 && code <= 0x200a) ||
        code === 0x2028 ||
        code === 0x2029 ||
        code === 0x202f ||
        code === 0x205f ||
        code === 0x3000 ||
        code === 0xfeff
    );
}

/**
 * The tokenizer splits text into pieces and merges bytes only within a piece. A non-whitespace
 * character ends every non-whitespace piece, and a whitespace run followed by other text keeps its
 * last character as a separate piece. When the next section starts with non-whitespace, `section`
 * plus the separator therefore contributes the tokens of its non-whitespace head, of its trailing
 * whitespace run plus one newline as one piece, and of the final newline piece. The result is
 * `null` for a section that is not well-formed.
 */
function separatedTokens(section: string, newlineTokens: number): number | null {
    if (!isWellFormed(section)) return null;
    let head = section.length;
    while (head > 0 && isTokenizerWhitespace(section.charCodeAt(head - 1))) head -= 1;
    if (head === section.length) return estimateTokens(section) + 2 * newlineTokens;
    const headTokens = head === 0 ? 0 : estimateTokens(section.slice(0, head));
    return headTokens + estimateTokens(`${section.slice(head)}\n`) + newlineTokens;
}

/** `kept` result blocks render, followed by the omission notice when `kept` is below the result count. */
interface Fit {
    kept: number;
    tokenCount: number;
}

/**
 * Sums per-section counts, which equal the assembled count because every section after the lead
 * starts with non-whitespace. The scan stops at the first block that overflows the budget, since
 * no longer candidate and no full text can fit after it. Each kept block adds more tokens than the
 * omission notice can shed, so the first candidate that fits going down is the largest. The result
 * is `null` when a section is not well-formed.
 */
function fitBySections(
    head: readonly string[],
    total: number,
    blockAt: (index: number) => string,
    noticeFor: (omitted: number) => string,
): Fit | null {
    const newlineTokens = estimateTokens("\n");
    let used = 0;
    for (const section of head) {
        const tokens = separatedTokens(section, newlineTokens);
        if (tokens === null) return null;
        used += tokens;
    }
    // `prefix[kept]` counts the head and `kept` blocks, each followed by a separator.
    const prefix = [used];
    while (prefix.length < total) {
        const tokens = separatedTokens(blockAt(prefix.length - 1), newlineTokens);
        if (tokens === null) return null;
        if (used + tokens > MAX_RENDERED_RESULT_TOKENS) break;
        used += tokens;
        prefix.push(used);
    }
    if (prefix.length === total) {
        const last = blockAt(total - 1);
        if (!isWellFormed(last)) return null;
        const tokenCount = used + estimateTokens(last);
        if (tokenCount <= MAX_RENDERED_RESULT_TOKENS) return { kept: total, tokenCount };
    }
    for (let kept = prefix.length - 1; kept >= 1; kept -= 1) {
        const tokenCount = (prefix[kept] as number) + estimateTokens(noticeFor(total - kept));
        if (tokenCount <= MAX_RENDERED_RESULT_TOKENS) return { kept, tokenCount };
    }
    return { kept: 0, tokenCount: (prefix[0] as number) + estimateTokens(noticeFor(total)) };
}

/** Text up to this many UTF-16 code units is counted whole first: memory text averages more than 3 code units per token, so such text usually fits. */
const WHOLE_TEXT_UNITS = 3 * MAX_RENDERED_RESULT_TOKENS;

/** Counts the full text in one pass when it is short enough to fit. The result is `null` when the text is longer than `WHOLE_TEXT_UNITS` or its count exceeds the budget. */
function fitWhole(
    head: readonly string[],
    total: number,
    blockAt: (index: number) => string,
): { text: string; tokenCount: number } | null {
    const sections = [...head];
    let units = 0;
    for (const section of head) units += section.length;
    for (let index = 0; index < total; index += 1) {
        const block = blockAt(index);
        units += block.length;
        if (units > WHOLE_TEXT_UNITS) return null;
        sections.push(block);
    }
    const text = assemble(sections);
    const tokenCount = estimateTokens(text);
    return tokenCount <= MAX_RENDERED_RESULT_TOKENS ? { text, tokenCount } : null;
}

/** Counts each assembled candidate whole: the full text, then a binary search over kept prefixes. */
function fitByText(total: number, count: (kept: number) => number): Fit {
    const fullTokens = count(total);
    if (fullTokens <= MAX_RENDERED_RESULT_TOKENS) return { kept: total, tokenCount: fullTokens };
    let fit: Fit = { kept: 0, tokenCount: -1 };
    binarySearchLargestFit(total - 2, (index) => {
        const tokenCount = count(index + 1);
        if (tokenCount > MAX_RENDERED_RESULT_TOKENS) return false;
        fit = { kept: index + 1, tokenCount };
        return true;
    });
    return fit.tokenCount < 0 ? { kept: 0, tokenCount: count(0) } : fit;
}

export type ExplicitDeliveryReason = "delivered" | "empty-results" | "packer-empty";

/** The `delivered` array contains exactly the results whose complete blocks appear in `text`, in rendered order.
 * */
export interface PackedSearchResults {
    text: string;
    delivered: KernelMemorySearchResult[];
    tokenCount: number;
    omittedCount: number;
    reason: ExplicitDeliveryReason;
}

/**
 * The packer appends an omission notice when it excludes result blocks.
 * Empty results and failure to fit any block produce an empty delivery rather than an error.
 *
 * `preamble` renders ahead of the results and counts against `MAX_RENDERED_RESULT_TOKENS`;
 * `tokenCount` includes `preamble` and all rendered result content.
 */
export function packSearchResults(
    query: string,
    results: KernelMemorySearchResult[],
    preamble?: string,
): PackedSearchResults {
    const boundedQuery = boundDynamicField(query);
    const lead = preamble ? [preamble] : [];
    if (results.length === 0) {
        const text = assemble([
            ...lead,
            `No results found for "${boundedQuery}" in project memories.`,
        ]);
        return {
            text,
            delivered: [],
            tokenCount: estimateTokens(text),
            omittedCount: 0,
            reason: "empty-results",
        };
    }

    const header = `Found ${results.length} result${results.length === 1 ? "" : "s"} for "${boundedQuery}":`;
    const noticeFor = (omitted: number) =>
        `(${omitted} result${omitted === 1 ? "" : "s"} omitted to fit the output budget — refine the query or lower the limit)`;
    const blocks: string[] = [];
    const blockAt = (index: number): string => {
        blocks[index] ??= formatResult(results[index] as KernelMemorySearchResult, index + 1);
        return blocks[index];
    };
    const sectionsFor = (kept: number): string[] => {
        const sections = [...lead, header];
        for (let index = 0; index < kept; index += 1) sections.push(blockAt(index));
        if (kept < results.length) sections.push(noticeFor(results.length - kept));
        return sections;
    };
    const whole = fitWhole([...lead, header], results.length, blockAt);
    if (whole) {
        return {
            text: whole.text,
            delivered: [...results],
            tokenCount: whole.tokenCount,
            omittedCount: 0,
            reason: "delivered",
        };
    }
    const fit =
        fitBySections([...lead, header], results.length, blockAt, noticeFor) ??
        fitByText(results.length, (kept) => estimateTokens(assemble(sectionsFor(kept))));
    return {
        text: assemble(sectionsFor(fit.kept)),
        delivered: results.slice(0, fit.kept),
        tokenCount: fit.tokenCount,
        omittedCount: results.length - fit.kept,
        reason: fit.kept === 0 ? "packer-empty" : "delivered",
    };
}
