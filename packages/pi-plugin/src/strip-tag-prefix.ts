import { stripPersistedAssistantText } from "@eidnara/opencode/hooks/context/tag-content-primitives";

const TAG_SECTION_CHAR = "\u00a7";
const LEADING_WHITESPACE_REGEX = /^\s*/;
const TRAILING_WHITESPACE_REGEX = /\s*$/;

interface TextPart {
    type: "text";
    text: string;
}

function isTextPart(part: unknown): part is TextPart {
    return (
        part !== null &&
        typeof part === "object" &&
        (part as { type?: unknown }).type === "text" &&
        typeof (part as { text?: unknown }).text === "string"
    );
}

/**
 * Whitespace at interior part boundaries separates words after parts are joined.
 * Only the first text part loses leading whitespace and only the last loses trailing whitespace.
 * Parts without a `§` carry no tag notation and stay untouched.
 * A part reduced to nothing but tag notation becomes empty.
 *
 * Mutates in place; returns `true` after modifying at least one part.
 */
export function stripTagPrefixFromAssistantMessage(message: {
    role: string;
    content: unknown;
}): boolean {
    if (message.role !== "assistant") return false;
    if (!Array.isArray(message.content)) return false;

    const textParts = message.content.filter(isTextPart);
    const last = textParts.length - 1;
    let mutated = false;

    for (let index = 0; index <= last; index++) {
        const textPart = textParts[index];
        const text = textPart.text;
        if (!text.includes(TAG_SECTION_CHAR)) continue;

        const core = stripPersistedAssistantText(text);
        const leading = index === 0 ? "" : (LEADING_WHITESPACE_REGEX.exec(text)?.[0] ?? "");
        const trailing = index === last ? "" : (TRAILING_WHITESPACE_REGEX.exec(text)?.[0] ?? "");
        const stripped = core.length === 0 ? "" : leading + core + trailing;

        if (stripped !== text) {
            textPart.text = stripped;
            mutated = true;
        }
    }

    return mutated;
}
