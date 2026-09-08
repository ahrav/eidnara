import { OMO_INTERNAL_INITIATOR_MARKER } from "./internal-initiator-marker";

// Eidnara, Oh My OpenCode, and Oh My Claude directives share this prefix.
const SYSTEM_DIRECTIVE_PREFIX = "[SYSTEM DIRECTIVE:";

// Notices end at the next blank line.
const SYSTEM_NOTICE_MARKERS = [
    "[Category+Skill Reminder]",
    "[EDIT ERROR - IMMEDIATE ACTION REQUIRED]",
    "[task CALL FAILED",
    "[EMERGENCY CONTEXT WINDOW WARNING]",
    "Unstable background agent appears idle",
    "**THE SUBAGENT JUST CLAIMED THIS TASK IS DONE.",
] as const;

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

export function removeSystemInjections(text: string): string {
    let cleaned = removeSystemReminders(text).replaceAll(OMO_INTERNAL_INITIATOR_MARKER, "");
    cleaned = removeBlocksStartingWith(cleaned, SYSTEM_DIRECTIVE_PREFIX, directiveBlockEnd);
    for (const marker of SYSTEM_NOTICE_MARKERS) {
        cleaned = removeBlocksStartingWith(cleaned, marker, (source, start) =>
            blankLineEnd(source, start + marker.length),
        );
    }
    // Cutting a whole paragraph leaves the blank lines on both sides of it adjacent.
    return cleaned.replace(/((?:\r?\n){2})(?:\r?\n)+/g, "$1").trim();
}

/** Collecting kept segments avoids copying the remaining tail for each marker. `endOf` must return an index past `start`. */
function removeBlocksStartingWith(
    text: string,
    marker: string,
    endOf: (text: string, start: number) => number,
): string {
    let start = text.indexOf(marker);
    if (start === -1) return text;
    const kept: string[] = [];
    let cursor = 0;
    while (start !== -1) {
        kept.push(text.slice(cursor, start));
        cursor = endOf(text, start);
        start = text.indexOf(marker, cursor);
    }
    kept.push(text.slice(cursor));
    return kept.join("");
}

/** A directive block includes its header and body; blank lines before list items remain part of the body. */
function directiveBlockEnd(text: string, start: number): number {
    const close = text.indexOf("]", start);
    if (close === -1) return text.length;
    let searchFrom = close + 1;
    for (;;) {
        const blank = findBlankLine(text, searchFrom);
        if (blank === null) return text.length;
        const next = text.slice(blank.end).trimStart()[0];
        if (next !== "-" && next !== "*") return blank.start;
        searchFrom = blank.end;
    }
}

function blankLineEnd(text: string, from: number): number {
    return findBlankLine(text, from)?.start ?? text.length;
}

function findBlankLine(text: string, from: number): { start: number; end: number } | null {
    const pattern = /\r?\n\r?\n/g;
    pattern.lastIndex = from;
    const match = pattern.exec(text);
    return match === null ? null : { start: match.index, end: match.index + match[0].length };
}
