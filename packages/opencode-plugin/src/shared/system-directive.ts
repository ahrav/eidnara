const SYSTEM_DIRECTIVE_PREFIX = "[SYSTEM DIRECTIVE: EIDNARA";

const SYSTEM_REMINDER_OPEN = "<system-reminder>";
const SYSTEM_REMINDER_CLOSE = "</system-reminder>";

export function isSystemDirective(text: string): boolean {
    return text.trimStart().startsWith(SYSTEM_DIRECTIVE_PREFIX);
}

/**
 * Tracks nesting depth so `<a><b>x</b> y</a>` drops `y` and the outer closer too; a non-greedy
 * regex stops at the first closer and leaves ` y</system-reminder>` in the output. An unmatched
 * closer at depth zero is dropped rather than echoed.
 */
export function removeSystemReminders(text: string): string {
    const lower = text.toLowerCase();
    let output = "";
    let depth = 0;
    let offset = 0;
    while (offset < text.length) {
        if (lower.startsWith(SYSTEM_REMINDER_OPEN, offset)) {
            depth += 1;
            offset += SYSTEM_REMINDER_OPEN.length;
            continue;
        }
        if (lower.startsWith(SYSTEM_REMINDER_CLOSE, offset)) {
            depth = Math.max(0, depth - 1);
            offset += SYSTEM_REMINDER_CLOSE.length;
            continue;
        }
        const codePoint = text.codePointAt(offset) as number;
        const width = codePoint > 0xffff ? 2 : 1;
        if (depth === 0) {
            output += text.slice(offset, offset + width);
        }
        offset += width;
    }
    return output.trim();
}
