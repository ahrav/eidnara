import { estimateTokens } from "../../shared/token-estimator";

/** The query validator checks the fused route's 4 KiB UTF-8 limit before trimming or tokenization. */
export const MAX_QUERY_BYTES = 4096;
export const MAX_QUERY_TOKENS = 512;
/** The memory ranker scans every operand against every served row, so the operand count bounds its work; the fused route's lexical lane admits 16. */
export const MAX_QUERY_ATOMS = 16;
/** Missing or non-finite result-limit requests default to 10. */
export const DEFAULT_SEARCH_RESULT_LIMIT = 10;
/** The fused route serves at most 32 entries. */
export const MAX_SEARCH_RESULT_LIMIT = 32;
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

/**
 * Runs of letters, digits, and underscores are operands; every other character separates them.
 * `QUERY_OPERAND_SEPARATOR` is a coarse pre-check, not the daemon's atom rule.
 */
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
    // An empty query is reported as missing by the caller; it needs no atom or token count.
    if (trimmed === "") return { ok: true, query: trimmed };
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

/** The cast supplies the `String.prototype.isWellFormed` signature for ES2022 library typings. */
export function isWellFormed(text: string): boolean {
    return (text as string & { isWellFormed(): boolean }).isWellFormed();
}

const utf8Encoder = new TextEncoder();
let utf8Scratch = new Uint8Array(MAX_RENDER_FIELD_BYTES);

/** `truncateUtf8Bytes` returns the longest prefix of `text` that preserves surrogate pairs and fits within `maxBytes` UTF-8 bytes. */
export function truncateUtf8Bytes(text: string, maxBytes: number): string {
    // A UTF-16 code unit encodes to at most 3 bytes.
    if (text.length * 3 <= maxBytes) return text;
    // Every code unit encodes to at least one byte, so the cut falls within the first `maxBytes + 1` units.
    const window = text.length > maxBytes ? text.slice(0, maxBytes + 1) : text;
    if (Number.isSafeInteger(maxBytes) && maxBytes >= 0 && isWellFormed(window)) {
        // `Buffer.byteLength` is exact for well-formed text.
        if (text.length <= maxBytes && Buffer.byteLength(text, "utf8") <= maxBytes) return text;
        if (utf8Scratch.length < maxBytes) utf8Scratch = new Uint8Array(maxBytes);
        let { read } = utf8Encoder.encodeInto(window, utf8Scratch.subarray(0, maxBytes));
        // In a well-formed window a cut after a high surrogate splits a pair.
        if (read > 0 && (window.charCodeAt(read - 1) & 0xfc00) === 0xd800) read -= 1;
        return text.slice(0, read);
    }
    return text.slice(0, utf8PrefixEnd(text, maxBytes));
}

/** A surrogate pair encodes as one 4-byte code point; a lone surrogate encodes as 3-byte U+FFFD. The cut falls on a code point boundary. */
function utf8PrefixEnd(text: string, maxBytes: number): number {
    let bytes = 0;
    let end = 0;
    while (end < text.length) {
        const unit = text.charCodeAt(end);
        const pair = (unit & 0xfc00) === 0xd800 && (text.charCodeAt(end + 1) & 0xfc00) === 0xdc00;
        const size = pair ? 4 : unit < 0x80 ? 1 : unit < 0x800 ? 2 : 3;
        if (bytes + size > maxBytes) break;
        bytes += size;
        end += pair ? 2 : 1;
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
