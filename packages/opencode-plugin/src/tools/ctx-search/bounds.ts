import { estimateTokens } from "../../shared/token-estimator";

/** The query validator checks the 16 KiB UTF-8 limit before trimming or tokenization. */
export const MAX_QUERY_BYTES = 16 * 1024;
export const MAX_QUERY_TOKENS = 512;
/** The memory ranker scans every operand against every served row, so the operand count bounds its work. */
export const MAX_QUERY_ATOMS = 64;
/** Missing or non-finite result-limit requests default to 10. */
export const DEFAULT_SEARCH_RESULT_LIMIT = 10;
export const MAX_SEARCH_RESULT_LIMIT = 50;
export const MAX_RENDERED_RESULT_TOKENS = 4096;
/** Renderers must apply the 1024-byte field limit before tokenization or compression. */
export const MAX_RENDER_FIELD_BYTES = 1024;

export function boundDynamicField(text: string): string {
    return truncateUtf8Bytes(text, MAX_RENDER_FIELD_BYTES);
}

export function renderAntiMemoryWarningLine(args: {
    trigger: string;
    rejectedStrategy: string;
    rejectionReason: string;
    saferAlternative: string | null | undefined;
    boundField: (text: string) => string;
    citation?: string;
}): string {
    // `boundField` can erase a non-empty alternative, so the renderer tests its output before adding the clause.
    const boundedAlternative = args.saferAlternative ? args.boundField(args.saferAlternative) : "";
    const alternative =
        boundedAlternative.length > 0 ? ` Safer alternative: ${boundedAlternative}.` : "";
    const citation = args.citation ? ` (see ${args.citation})` : "";
    return `⚠ Previously rejected: ${args.boundField(args.rejectedStrategy)}. Reason: ${args.boundField(args.rejectionReason)}.${alternative} Verify before proceeding: confirm the rejection no longer applies to ${args.boundField(args.trigger)}.${citation}`;
}

export type QueryBoundsViolation = "bytes" | "tokens" | "atoms";

export interface QueryBoundsDetail {
    violation: QueryBoundsViolation;
    limit: number;
    actual: number;
}

export class QueryBoundsError extends Error {
    readonly violation: QueryBoundsViolation;
    readonly limit: number;
    readonly actual: number;

    constructor(detail: QueryBoundsDetail) {
        super(describeQueryBoundsViolation(detail));
        this.name = "QueryBoundsError";
        this.violation = detail.violation;
        this.limit = detail.limit;
        this.actual = detail.actual;
    }
}

export function describeQueryBoundsViolation(detail: QueryBoundsDetail): string {
    switch (detail.violation) {
        case "bytes":
            return `query is too large: ${detail.actual} bytes of UTF-8 exceeds the ${detail.limit}-byte maximum`;
        case "tokens":
            return `query is too large: ~${detail.actual} estimated tokens exceeds the ${detail.limit}-token maximum`;
        case "atoms":
            return `query is too complex: ${detail.actual} search terms exceeds the ${detail.limit}-term maximum`;
    }
}

/** Runs of letters, digits, and underscores are operands; every other character separates them. */
const QUERY_OPERAND_SEPARATOR = /[^\p{L}\p{N}_]+/u;

/** The operands of `query` in order, duplicates kept. */
export function splitQueryOperands(query: string): string[] {
    return query.split(QUERY_OPERAND_SEPARATOR).filter(Boolean);
}

/** Duplicates count separately so the count is an upper bound on the ranker's deduplicated term set. */
export function countQueryAtoms(query: string): number {
    return splitQueryOperands(query).length;
}

export type ExplicitQueryPreparation =
    | { ok: true; query: string }
    | ({ ok: false } & QueryBoundsDetail);

/**
 * The validator checks bytes before trimming so an over-cap whitespace-only query rejects without trim work.
 * Atom and token checks run on the trimmed query the search lanes would consume.
 */
export function prepareExplicitQuery(raw: string): ExplicitQueryPreparation {
    const bytes = Buffer.byteLength(raw, "utf8");
    if (bytes > MAX_QUERY_BYTES) {
        return { ok: false, violation: "bytes", limit: MAX_QUERY_BYTES, actual: bytes };
    }
    const trimmed = raw.trim();
    const atoms = countQueryAtoms(trimmed);
    if (atoms > MAX_QUERY_ATOMS) {
        return { ok: false, violation: "atoms", limit: MAX_QUERY_ATOMS, actual: atoms };
    }
    const tokens = estimateTokens(trimmed);
    if (tokens > MAX_QUERY_TOKENS) {
        return { ok: false, violation: "tokens", limit: MAX_QUERY_TOKENS, actual: tokens };
    }
    return { ok: true, query: trimmed };
}

/** The function returns the longest prefix of `text` whose UTF-8 encoding is at most `maxBytes` without splitting surrogate pairs. */
export function truncateUtf8Bytes(text: string, maxBytes: number): string {
    if (Buffer.byteLength(text, "utf8") <= maxBytes) return text;
    return text.slice(0, utf8PrefixEnd(text, maxBytes));
}

/** Iterating by code point keeps a surrogate pair as one 4-byte unit so the cut never lands inside it. */
function utf8PrefixEnd(text: string, maxBytes: number): number {
    let bytes = 0;
    let end = 0;
    for (const char of text) {
        const charBytes = Buffer.byteLength(char, "utf8");
        if (bytes + charBytes > maxBytes) break;
        bytes += charBytes;
        end += char.length;
    }
    return end;
}

/** The function returns the largest index in `[0, upper]` whose prefix satisfies monotone `fits`, or -1 when none fits. */
export function binarySearchLargestFit(upper: number, fits: (index: number) => boolean): number {
    let low = 0;
    let high = upper;
    let best = -1;
    while (low <= high) {
        const mid = (low + high) >> 1;
        if (fits(mid)) {
            best = mid;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }
    return best;
}

/**
 * Missing or non-finite values use the default; finite values are floored and clamped to [1, MAX_SEARCH_RESULT_LIMIT].
 */
export function normalizeSearchResultLimit(limit?: number): number {
    if (typeof limit !== "number" || !Number.isFinite(limit)) {
        return DEFAULT_SEARCH_RESULT_LIMIT;
    }
    return Math.min(MAX_SEARCH_RESULT_LIMIT, Math.max(1, Math.floor(limit)));
}
