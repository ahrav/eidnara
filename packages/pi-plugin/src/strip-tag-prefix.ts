import { isTextPart, replacedContent, stripTags } from "./stream-tag-strip";

const TAG_SECTION_CHAR = "\u00a7";

/**
 * Strips each text part as complete text by the rule the stream applies (`stripTags`): every
 * `§N§` tag and mangled `§N">` tag is removed, and the first text part also loses its opening
 * tags with the whitespace around them. All other text stays, whitespace and every other `§`
 * included, so the saved answer is the text the stream showed at its end.
 * Parts without a `§` carry no tag notation and stay untouched.
 * When `message.content` is the snapshot `message_start` installed, which happens when Pi hands
 * both events the same message object, the content it replaced is put back and stripped.
 *
 * Mutates in place; returns `true` after modifying at least one part.
 */
export function stripTagPrefixFromAssistantMessage(message: {
    role: string;
    content: unknown;
}): boolean {
    if (message.role !== "assistant") return false;
    message.content = replacedContent(message.content) ?? message.content;
    if (!Array.isArray(message.content)) return false;

    const textParts = message.content.filter(isTextPart);
    const last = textParts.length - 1;
    let mutated = false;

    for (let index = 0; index <= last; index++) {
        const textPart = textParts[index];
        const text = textPart.text;
        if (!text.includes(TAG_SECTION_CHAR)) continue;

        const stripped = stripTags(text, index === 0);
        if (stripped !== text) {
            textPart.text = stripped;
            mutated = true;
        }
    }

    return mutated;
}
