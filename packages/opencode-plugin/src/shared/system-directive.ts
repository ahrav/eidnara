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
    cleaned = removeDirectiveBlocks(cleaned);
    for (const marker of SYSTEM_NOTICE_MARKERS) {
        cleaned = removeBlocksStartingWith(cleaned, marker, (body) => blankLineEnd(body, 0));
    }
    // Cutting a whole paragraph leaves the blank lines on both sides of it adjacent.
    return cleaned.replace(/\n{3,}/g, "\n\n").trim();
}

/** A directive block includes its header and body; blank lines before list items remain part of the body. */
function removeDirectiveBlocks(text: string): string {
    return removeBlocksStartingWith(text, SYSTEM_DIRECTIVE_PREFIX, (body) => {
        const close = body.indexOf("]");
        return close === -1 ? body.length : directiveBodyEnd(body, close + 1);
    });
}

function removeBlocksStartingWith(
    text: string,
    marker: string,
    endOf: (body: string) => number,
): string {
    let cleaned = text;
    let searchFrom = 0;
    for (;;) {
        const start = cleaned.indexOf(marker, searchFrom);
        if (start === -1) return cleaned;
        const end = start + endOf(cleaned.slice(start));
        cleaned = cleaned.slice(0, start) + cleaned.slice(end);
        searchFrom = start;
    }
}

function directiveBodyEnd(text: string, bodyStart: number): number {
    let searchFrom = bodyStart;
    for (;;) {
        const boundary = text.indexOf("\n\n", searchFrom);
        if (boundary === -1) return text.length;
        const next = text.slice(boundary + 2).trimStart()[0];
        if (next !== "-" && next !== "*") return boundary;
        searchFrom = boundary + 2;
    }
}

function blankLineEnd(text: string, from: number): number {
    const boundary = text.indexOf("\n\n", from);
    return boundary === -1 ? text.length : boundary;
}
