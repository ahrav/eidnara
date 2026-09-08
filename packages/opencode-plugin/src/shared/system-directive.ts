// Eidnara, Oh My OpenCode, and Oh My Claude directives share this prefix.
const SYSTEM_DIRECTIVE_PREFIX = "[SYSTEM DIRECTIVE:";

export function isSystemDirective(text: string): boolean {
    return text.trimStart().startsWith(SYSTEM_DIRECTIVE_PREFIX);
}

function createSystemReminderTagPattern(): RegExp {
    return /<(\/?)system-reminder>/gi;
}

/**
 * Tracks nesting depth so `<a><b>x</b> y</a>` drops `y` and the outer closer too; a non-greedy
 * regex stops at the first closer and leaves ` y</system-reminder>` in the output. An unmatched
 * closing tag at depth zero is dropped. The pattern matches the original string because case
 * folding can change character length, making lowercased-string indices invalid.
 */
export function removeSystemReminders(text: string): string {
    let output = "";
    let depth = 0;
    let consumed = 0;
    for (const match of text.matchAll(createSystemReminderTagPattern())) {
        if (depth === 0) {
            output += text.slice(consumed, match.index);
        }
        depth = match[1] === "/" ? Math.max(0, depth - 1) : depth + 1;
        consumed = match.index + match[0].length;
    }
    if (depth === 0) {
        output += text.slice(consumed);
    }
    return output.trim();
}
