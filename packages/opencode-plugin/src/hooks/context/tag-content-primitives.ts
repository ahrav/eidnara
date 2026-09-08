/** A reasoning-bearing part as the harness delivers it: `thinking` or `text` carries the content. */
export interface ThinkingLikePart {
    type: string;
    thinking?: string;
    text?: string;
}

const encoder = new TextEncoder();

const TAG_PREFIX_REGEX = /^(?:§\d+§\s*)+/;

//
//
// `§15298">§15298§ hello...` matches the malformed-prefix regex.
// `MALFORMED_TAG_PREFIX_REGEX` matches `§15298">§ hello...` without closing digits.
//
//
const MALFORMED_TAG_PREFIX_REGEX = /^(?:§\d+">§(?:\d+§)?\s*)+/;

// `(?!\d|\.\d)` rejects decimal references and prevents `\d+` from backtracking to a shorter prefix.
// Without the digit alternative, `§42.1` matches as `§4` and leaves `2.1`.
// The optional closer excludes Unicode letters and numbers except `\u04a9`; JavaScript's `\w` is ASCII-only,
// so `[^\w]` would consume the first character of `§42修复完成` or `§42éclair`.
// `\u04a9` (ҩ) is a letter a model has been observed to improvise as a closer, so it is listed explicitly.
const DANGLING_TAG_GLOBAL_REGEX = /\u00a7\d+(?!\d|\.\d)(?:[^\s\u00a7\p{L}\p{N}.]|\u04a9)?/gu;
const DANGLING_TAG_PREFIX_REGEX =
    /^(?:\u00a7\d+(?!\d|\.\d)(?:[^\s\u00a7\p{L}\p{N}.]|\u04a9)?\s*)+/u;

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
 * The function removes whole `§N§` pairs.
 * It never removes bare digits; it then removes malformed tag notation and stray `§`.
 */
export function stripPersistedAssistantText(value: string): string {
    let text = stripWellFormedLeadingTagPrefix(value);
    text = stripCompleteTagPairsGlobally(text);
    text = stripMalformedTagNotationGlobally(text);
    // stripDanglingTagNotationGlobally runs before stripTagSectionCharacters so `§N$` and `§Nҩ` are removed as units.
    text = stripDanglingTagNotationGlobally(text);
    text = stripTagSectionCharacters(text);
    return text.trim();
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
    // Every replacement removes at least one character, so the loop ends within `value.length` passes.
    for (;;) {
        const prev = stripped;
        stripped = stripped.replace(MALFORMED_TAG_PREFIX_REGEX, "");
        stripped = stripped.replace(TAG_PREFIX_REGEX, "");
        // A removed well-formed prefix can expose a malformed one, as in `§1§ §2">§2§ x`. Restarting
        // lets the malformed pass remove it whole; the dangling pass would take only `§2"` and leave `>§2§ x`.
        if (stripped !== prev) continue;
        // The dangling pass runs only when no well-formed prefix remains, so `§N§` is never reduced to `§`.
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
