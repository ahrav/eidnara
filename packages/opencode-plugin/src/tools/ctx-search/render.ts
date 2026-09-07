/**
 * This module renders explicit `ctx_search` output under a token budget.
 *
 * The renderer bounds each dynamic result field to `MAX_DYNAMIC_FIELD_BYTES` of valid UTF-8 before tokenization.
 * Packing retains a ranked prefix of complete result blocks under `MAX_RENDERED_RESULT_TOKENS`.
 */

import { estimateTokens } from "../../shared/token-estimator";
import {
    binarySearchLargestFit,
    boundDynamicField,
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
        citation: result.publicClaimId,
    });
}

function formatResult(result: KernelMemorySearchResult, index: number): string {
    if (result.source === "anti_memory") {
        const policy = result.policyLabel
            ? ` status=${boundDynamicField(result.policyLabel)}`
            : " status=active";
        return [
            `[${index}] [anti-memory warning] score=${result.score.toFixed(2)} id=${result.publicClaimId} match=${result.matchType}${policy}`,
            renderAntiMemoryWarning(result),
            // The rationale renders only in this full-result view; the compact auto-search hint keeps the warning-line field set.
            ...(result.rationale ? [`Rationale: ${boundDynamicField(result.rationale)}`] : []),
        ].join("\n");
    }
    const source = result.sourceName ? ` source=${boundDynamicField(result.sourceName)}` : "";
    const policy = result.policyLabel ? ` trust=[${boundDynamicField(result.policyLabel)}]` : "";
    return [
        `[${index}] [memory] score=${result.score.toFixed(2)} id=${result.publicClaimId} category=${boundDynamicField(result.category)}${source} match=${result.matchType}${policy}`,
        boundDynamicField(result.content),
    ].join("\n");
}

function assemble(header: string, parts: readonly string[]): string {
    return `${header}\n\n${parts.join("\n\n")}`;
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
 */
export function packSearchResults(
    query: string,
    results: KernelMemorySearchResult[],
): PackedSearchResults {
    const boundedQuery = boundDynamicField(query);
    if (results.length === 0) {
        const text = `No results found for "${boundedQuery}" in project memories.`;
        return {
            text,
            delivered: [],
            tokenCount: estimateTokens(text),
            omittedCount: 0,
            reason: "empty-results",
        };
    }

    const header = `Found ${results.length} result${results.length === 1 ? "" : "s"} for "${boundedQuery}":`;
    const blocks = results.map((result, index) => formatResult(result, index + 1));

    const full = assemble(header, blocks);
    const fullTokens = estimateTokens(full);
    if (fullTokens <= MAX_RENDERED_RESULT_TOKENS) {
        return {
            text: full,
            delivered: [...results],
            tokenCount: fullTokens,
            omittedCount: 0,
            reason: "delivered",
        };
    }

    const noticeFor = (omitted: number) =>
        `(${omitted} result${omitted === 1 ? "" : "s"} omitted to fit the output budget — refine the query or lower the limit)`;
    const candidateFor = (kept: number) =>
        assemble(header, [...blocks.slice(0, kept), noticeFor(results.length - kept)]);

    const bestIndex = binarySearchLargestFit(
        results.length - 2,
        (index) => estimateTokens(candidateFor(index + 1)) <= MAX_RENDERED_RESULT_TOKENS,
    );
    if (bestIndex >= 0) {
        const kept = bestIndex + 1;
        const text = candidateFor(kept);
        return {
            text,
            delivered: results.slice(0, kept),
            tokenCount: estimateTokens(text),
            omittedCount: results.length - kept,
            reason: "delivered",
        };
    }

    const text = assemble(header, [noticeFor(results.length)]);
    return {
        text,
        delivered: [],
        tokenCount: estimateTokens(text),
        omittedCount: results.length,
        reason: "packer-empty",
    };
}
