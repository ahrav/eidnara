/** A reasoning-bearing part as the harness delivers it: `thinking` or `text` carries the content. */
export interface ThinkingLikePart {
    type: string;
    thinking?: string;
    text?: string;
}

export type MessageInfo = {
    id?: string;
    role?: string;
    sessionID?: string;
    summary?: boolean;
    /** syntheticHead marks one of the two m[0]/m[1] messages prepended by compartment injection. */
    syntheticHead?: boolean;
    finish?: string;
    error?: unknown;
};

export type MessageLike = { info: MessageInfo; parts: unknown[] };

const encoder = new TextEncoder();

const TAG_PREFIX_REGEX = /^(?:§\d+§\s*)+/;

//
//
// `§15298">§15298§ hello...` matches the malformed-prefix regex.
// `MALFORMED_TAG_PREFIX_REGEX` matches `§15298">§ hello...` without closing digits.
//
//
const MALFORMED_TAG_PREFIX_REGEX = /^(?:§\d+">§(?:\d+§)?\s*)+/;

//
//
// The negative lookahead rejects `§5.1`; the optional closer does not consume whitespace, `§`, word characters, or `.`.
// `(?!\d)` pins `\d+` to the whole digit run, so `§12.3` cannot backtrack to `§1` and pass the decimal lookahead on `2.3`.
// The closer does not consume an ASCII word character or `.`.
// The closer preserves word characters, periods, and whitespace (`§42important` → `important`).
const DANGLING_TAG_GLOBAL_REGEX = /\u00a7\d+(?!\d)(?!\.\d)[^\s\u00a7\w.]?/g;
const DANGLING_TAG_PREFIX_REGEX = /^(?:\u00a7\d+(?!\d)(?!\.\d)[^\s\u00a7\w.]?\s*)+/;

/* */
const COMPLETE_TAG_PAIR_GLOBAL_REGEX = /\u00a7\d+\u00a7/g;

/* */
const MALFORMED_TAG_GLOBAL_REGEX = /\u00a7\d+">(?:\u00a7(?:\d+\u00a7)?)?/g;

/** Lone section signs removed after pair and malformed-notation removal. */
const STRAY_SECTION_CHAR_REGEX = /\u00a7/g;

export function stripWellFormedLeadingTagPrefix(value: string): string {
    return value.replace(/^(\u00a7\d+\u00a7\s*)+/, "");
}

function stripCompleteTagPairsGlobally(value: string): string {
    return value.replace(COMPLETE_TAG_PAIR_GLOBAL_REGEX, "");
}

function stripMalformedTagNotationGlobally(value: string): string {
    return value.replace(MALFORMED_TAG_GLOBAL_REGEX, "");
}

/* */
export function stripDanglingTagNotationGlobally(value: string): string {
    return value.replace(DANGLING_TAG_GLOBAL_REGEX, "");
}

export function stripTagSectionCharacters(value: string): string {
    return value.replace(STRAY_SECTION_CHAR_REGEX, "");
}

/**
 * The function removes tag notation wherever it appears and leaves the surrounding whitespace in
 * place, so a caller that owns the text's boundaries decides what to trim.
 */
export function stripTagNotationGlobally(value: string): string {
    let text = stripCompleteTagPairsGlobally(value);
    text = stripMalformedTagNotationGlobally(text);
    // stripDanglingTagNotationGlobally runs before stripTagSectionCharacters so `§N$` and `§Nҩ` are removed as units.
    text = stripDanglingTagNotationGlobally(text);
    return stripTagSectionCharacters(text);
}

/**
 * The function removes whole `§N§` pairs.
 * It never removes bare digits; it then removes malformed tag notation and stray `§`.
 */
export function stripPersistedAssistantText(value: string): string {
    return stripTagNotationGlobally(stripWellFormedLeadingTagPrefix(value)).trim();
}

export function byteSize(value: string): number {
    return encoder.encode(value).length;
}

/**
 * The function strips §-shaped tag notation only from the start of text.
 * Does not remove bare leading digits: preserve `99 files`, `2024 roadmap`, and numbered lists.
 */
export function stripTagPrefix(value: string): string {
    let stripped = value;
    for (let pass = 0; pass < 8; pass++) {
        const prev = stripped;
        stripped = stripped.replace(MALFORMED_TAG_PREFIX_REGEX, "");
        stripped = stripped.replace(TAG_PREFIX_REGEX, "");
        // Run `DANGLING_TAG_PREFIX_REGEX` after `TAG_PREFIX_REGEX` so a well-formed `§N§` is not reduced to `§`.
        // `DANGLING_TAG_PREFIX_REGEX` would otherwise strip `§N` and leave the closing `§`.
        stripped = stripped.replace(DANGLING_TAG_PREFIX_REGEX, "");
        if (stripped === prev) break;
    }
    return stripped;
}

/**
 */
export function peelLeadingMcTagNotation(value: string): { tagPrefix: string; body: string } {
    const body = stripTagPrefix(value);
    if (body === value) return { tagPrefix: "", body };
    return { tagPrefix: value.slice(0, value.length - body.length), body };
}

export function prependTag(tagId: number, value: string): string {
    const stripped = stripTagPrefix(value);
    return `§${tagId}§ ${stripped}`;
}

export function isThinkingPart(part: unknown): part is ThinkingLikePart {
    if (part === null || typeof part !== "object") return false;
    const candidate = part as Record<string, unknown>;
    return candidate.type === "thinking" || candidate.type === "reasoning";
}
