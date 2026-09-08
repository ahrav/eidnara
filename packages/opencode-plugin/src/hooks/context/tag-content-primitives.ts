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

// The lookahead preserves numeric references such as `§42.1` and adjacent tags such as `§1 §2§`.
// The closed closer set prevents a broad symbol class from consuming an authored emoji or a `#` heading marker.
const DANGLING_TAG_CORE = String.raw`\u00a7\d+(?!\d|\.\d|\u00a7)(?:">|[$"'\u04a9])?`;
const DANGLING_TAG_GLOBAL_REGEX = new RegExp(DANGLING_TAG_CORE, "gu");

// Sticky (`y`) makes each rule match only at the current offset. The malformed rule precedes the
// dangling rule because both can match `§N">`, and only the malformed rule consumes the whole hybrid.
const LEADING_TAG_RULES: readonly RegExp[] = [
    /\u00a7\d+">\u00a7(?:\d+\u00a7)?[\s\u0085]*/y,
    /\u00a7\d+\u00a7[\s\u0085]*/y,
    new RegExp(String.raw`${DANGLING_TAG_CORE}[\s\u0085]*`, "uy"),
];

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
    // Each accepted match advances `offset`, preventing zero-length matches from looping indefinitely.
    let offset = 0;
    scan: while (offset < value.length) {
        for (const rule of LEADING_TAG_RULES) {
            rule.lastIndex = offset;
            const match = rule.exec(value);
            if (match !== null && match[0].length > 0) {
                offset += match[0].length;
                continue scan;
            }
        }
        break;
    }
    return offset === 0 ? value : value.slice(offset);
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
