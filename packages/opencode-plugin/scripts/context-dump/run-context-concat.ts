import Tokenizer from "ai-tokenizer";
import * as claude from "ai-tokenizer/encoding/claude";
import { resolveOpenCodeDatabasePath } from "../../src/shared/opencode-database-path";
import { readOpenCodeSessionMessages } from "./read-opencode-session";
import type { DumpMessage } from "./types";

const tokenizer = new Tokenizer(claude);

export interface ConcatPage {
    totalMessages: number;
    startIndex: number;
    /** Index of the last message consumed into this page within the array that was read. */
    endIndex: number;
    messagesWithContent: number;
    /** `totalTokens` counts `output` as one joined string, not as a sum over lines. */
    totalTokens: number;
    hasMore: boolean;
    output: string;
}

/** Resuming after an anchor survives deletion of earlier messages; a numeric index would skip surviving messages. */
export interface MessageAnchor {
    timeCreated: number;
    id: string;
}

export interface ConcatResult extends ConcatPage {
    sessionId: string;
    /** Anchor of the last consumed message; `null` when the page consumed nothing. */
    endAnchor: MessageAnchor | null;
}

/** Index of the first message ordered after `anchor`, or `messages.length` when none is. */
export function indexAfterAnchor(messages: DumpMessage[], anchor: MessageAnchor): number {
    for (let i = 0; i < messages.length; i++) {
        const info = (messages[i] as DumpMessage).info;
        const timeCreated = info.timeCreated;
        const id = info.id;
        if (typeof timeCreated !== "number" || typeof id !== "string") continue;
        if (
            timeCreated > anchor.timeCreated ||
            (timeCreated === anchor.timeCreated && id > anchor.id)
        ) {
            return i;
        }
    }
    return messages.length;
}

function anchorOf(message: DumpMessage | undefined): MessageAnchor | null {
    if (!message) return null;
    const { timeCreated, id } = message.info;
    if (typeof timeCreated !== "number" || typeof id !== "string") return null;
    return { timeCreated, id };
}

export function formatAnchor(anchor: MessageAnchor): string {
    return `${anchor.timeCreated}:${anchor.id}`;
}

export function parseAnchor(raw: string): MessageAnchor {
    const match = /^(\d+):(.+)$/.exec(raw);
    if (!match) {
        throw new Error(`--after expects <timeCreated>:<messageId>, got ${JSON.stringify(raw)}`);
    }
    return { timeCreated: Number.parseInt(match[1] as string, 10), id: match[2] as string };
}

function countTokens(text: string): number {
    return tokenizer.count(text);
}

function extractTextParts(parts: unknown[]): string[] {
    const texts: string[] = [];
    for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        const p = part as Record<string, unknown>;
        if (p.type === "text" && typeof p.text === "string" && p.text.trim().length > 0) {
            texts.push(p.text.trim());
        }
    }
    return texts;
}

function countToolParts(parts: unknown[]): number {
    let count = 0;
    for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        const p = part as Record<string, unknown>;
        if (
            p.type === "tool" ||
            p.type === "tool-invocation" ||
            p.type === "tool_use" ||
            p.type === "tool_result"
        ) {
            count++;
        }
    }
    return count;
}

function capitalize(s: string): string {
    return s.charAt(0).toUpperCase() + s.slice(1);
}

function toolSummaryLine(firstIndex: number, toolCount: number): string {
    return `[${firstIndex}] Assistant: ${toolCount} tool call${toolCount > 1 ? "s" : ""}`;
}

/**
 * OpenCode creates an assistant message before its parts arrive and sets `time.completed` when the turn ends.
 * A trailing assistant message whose `time` lacks `completed` is still being written, with or without parts,
 * so consuming it would make the next page skip whatever text lands after this read.
 * A message with no `time` object carries no completion signal and is treated as complete.
 */
function isUnfinishedTrailingMessage(
    messages: DumpMessage[],
    index: number,
    role: string,
): boolean {
    if (role !== "assistant" || index !== messages.length - 1) return false;
    const time = (messages[index] as DumpMessage).info.time;
    if (time === null || typeof time !== "object") return false;
    return (time as { completed?: unknown }).completed == null;
}

/**
 * Admission tokenizes the joined output because newline separators and BPE
 * merges make per-line token counts non-additive.
 *
 * Consecutive assistant-only tool messages share one summary line. Their
 * indices count toward `endIndex` only once that summary line is admitted, so
 * a caller resuming after `endIndex` never skips a run whose summary line the
 * budget rejected.
 *
 * `lastIndex` remains `offset - 1` until a message is consumed.
 * When the first line of a page exceeds `tokenBudget`, `admit` throws:
 * resuming at the same offset would reject that line again.
 *
 * A caller that sees `hasMore` with no progress (`endIndex + 1 === offset`) should retry later with the same anchor rather than advance past an unfinished message.
 */
export function concatSessionMessages(
    messages: DumpMessage[],
    tokenBudget: number,
    offset = 0,
): ConcatPage {
    let output = "";
    let totalTokens = 0;
    let messagesWithContent = 0;
    let lastIndex = offset - 1;

    let pendingToolCount = 0;
    let pendingToolMessages = 0;
    let pendingToolFirstIndex = offset;
    let pendingToolLastIndex = offset;

    const admit = (index: number, text: string): boolean => {
        const candidate = output.length === 0 ? text : `${output}\n${text}`;
        const candidateTokens = countTokens(candidate);
        if (candidateTokens > tokenBudget) {
            if (output.length === 0) {
                throw new Error(
                    `Message ${index} needs ${candidateTokens} tokens on its own, which exceeds the budget of ${tokenBudget}; raise the budget`,
                );
            }
            return false;
        }
        output = candidate;
        totalTokens = candidateTokens;
        return true;
    };

    const admitPendingTools = (): boolean => {
        const line = toolSummaryLine(pendingToolFirstIndex, pendingToolCount);
        if (!admit(pendingToolFirstIndex, line)) return false;
        lastIndex = pendingToolLastIndex;
        messagesWithContent += pendingToolMessages;
        pendingToolCount = 0;
        pendingToolMessages = 0;
        return true;
    };

    for (let i = offset; i < messages.length; i++) {
        const msg = messages[i] as DumpMessage;
        const role = String(msg.info.role ?? "unknown");
        if (isUnfinishedTrailingMessage(messages, i, role)) break;
        const texts = extractTextParts(msg.parts);
        const toolCount = countToolParts(msg.parts);

        if (toolCount > 0 && texts.length === 0 && role === "assistant") {
            if (pendingToolCount === 0) pendingToolFirstIndex = i;
            pendingToolCount += toolCount;
            pendingToolMessages++;
            pendingToolLastIndex = i;
            continue;
        }

        if (pendingToolCount > 0 && !admitPendingTools()) break;

        if (texts.length === 0) {
            lastIndex = i;
            continue;
        }

        const prefix =
            toolCount > 0 ? ` (+ ${toolCount} tool call${toolCount > 1 ? "s" : ""})` : "";
        const line = `[${i}] ${capitalize(role)}${prefix}: ${texts.join("\n")}`;
        if (!admit(i, line)) break;

        messagesWithContent++;
        lastIndex = i;
    }

    if (pendingToolCount > 0) admitPendingTools();

    return {
        totalMessages: messages.length,
        startIndex: offset,
        endIndex: lastIndex,
        messagesWithContent,
        totalTokens,
        hasMore: lastIndex + 1 < messages.length,
        output,
    };
}

export function runContextConcat(
    sessionId: string,
    tokenBudget: number,
    after: MessageAnchor | null = null,
): ConcatResult {
    const opencodeDbPath = resolveOpenCodeDatabasePath();
    const allMessages = readOpenCodeSessionMessages(opencodeDbPath, sessionId);
    const offset = after ? indexAfterAnchor(allMessages, after) : 0;
    const page = concatSessionMessages(allMessages, tokenBudget, offset);
    const consumed = page.endIndex >= offset ? allMessages[page.endIndex] : undefined;
    return { sessionId, ...page, endAnchor: anchorOf(consumed) };
}

const USAGE = `Usage: bun scripts/context-dump/run-context-concat.ts <session-id> --budget <tokens> [--after <timeCreated>:<messageId>]

Prints one page of the session as JSON (see ConcatResult). Pass the printed
endAnchor as --after <timeCreated>:<messageId> to fetch the next page while
hasMore is true; the anchor resumes correctly even when earlier messages were
deleted in between. A page with hasMore true and endAnchor null ends at an
assistant message that is still being written; retry later with the same
--after instead of advancing. Exits 1 when the first message of a page needs
more tokens than --budget allows.
Set OPENCODE_DB_PATH to read a database other than the discovered default.`;

function parseNonNegativeInt(flag: string, raw: string | undefined): number {
    if (raw === undefined || !/^\d+$/.test(raw)) {
        throw new Error(`${flag} requires a non-negative integer, got ${JSON.stringify(raw)}`);
    }
    return Number.parseInt(raw, 10);
}

function parseArgs(argv: string[]): {
    sessionId: string;
    tokenBudget: number;
    after: MessageAnchor | null;
} {
    let sessionId: string | undefined;
    let tokenBudget: number | undefined;
    let after: MessageAnchor | null = null;
    for (let i = 0; i < argv.length; i++) {
        const arg = argv[i];
        if (arg === "--budget") {
            tokenBudget = parseNonNegativeInt(arg, argv[++i]);
        } else if (arg === "--after") {
            const raw = argv[++i];
            if (raw === undefined) throw new Error("--after requires <timeCreated>:<messageId>");
            after = parseAnchor(raw);
        } else if (arg.startsWith("--")) {
            throw new Error(`Unknown flag ${arg}`);
        } else if (sessionId === undefined) {
            sessionId = arg;
        } else {
            throw new Error(`Unexpected argument ${JSON.stringify(arg)}`);
        }
    }
    if (sessionId === undefined) throw new Error("Missing <session-id>");
    if (tokenBudget === undefined) throw new Error("Missing --budget <tokens>");
    return { sessionId, tokenBudget, after };
}

function main(): void {
    let args: ReturnType<typeof parseArgs>;
    try {
        args = parseArgs(process.argv.slice(2));
    } catch (err) {
        console.error(err instanceof Error ? err.message : String(err));
        console.error(USAGE);
        process.exit(2);
    }
    try {
        const result = runContextConcat(args.sessionId, args.tokenBudget, args.after);
        console.log(JSON.stringify(result, null, 2));
    } catch (err) {
        console.error(err instanceof Error ? err.message : String(err));
        process.exit(1);
    }
}

if (import.meta.main) {
    main();
}
