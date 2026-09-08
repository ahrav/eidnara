import {
    stripTagNotationGlobally,
    stripWellFormedLeadingTagPrefix,
} from "@eidnara/opencode/hooks/context/tag-content-primitives";

const TAG_SECTION_CHAR = "\u00a7";

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
 * Interior boundary whitespace separates joined words.
 * Every part keeps the whitespace around removed tag notation; only the first text part loses a
 * leading tag prefix with its following whitespace and only the last loses trailing whitespace.
 * Parts without a `§` carry no tag notation and stay untouched.
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

        let stripped = index === 0 ? stripWellFormedLeadingTagPrefix(text) : text;
        stripped = stripTagNotationGlobally(stripped);
        if (index === 0) stripped = stripped.trimStart();
        if (index === last) stripped = stripped.trimEnd();

        if (stripped !== text) {
            textPart.text = stripped;
            mutated = true;
        }
    }

    return mutated;
}
