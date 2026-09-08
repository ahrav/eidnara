import { COMMIT_VERB_PATTERN, createCommitHashExtractPattern } from "../../shared/commit-detection";
import { OMO_INTERNAL_INITIATOR_MARKER } from "../../shared/internal-initiator-marker";
import { isSystemInjectedText, removeSystemReminders } from "../../shared/system-directive";

export interface SessionChunkLine {
    ordinal: number;
    messageId: string;
}

export interface ChunkBlock {
    role: string;
    startOrdinal: number;
    endOrdinal: number;
    parts: string[];
    meta: SessionChunkLine[];
    commitHashes: string[];
    /**
     * isToolOnly is true when every part in the block came from a tool-call summary.
     */
    isToolOnly: boolean;
}

const MAX_COMMITS_PER_BLOCK = 5;

export function isTruthyFlag(value: unknown): boolean {
    return value === true || value === 1 || value === "true";
}

export function isMachineAuthoredPart(part: Record<string, unknown>): boolean {
    if (isTruthyFlag(part.synthetic) || isTruthyFlag(part.ignored)) return true;
    const metadata = part.metadata;
    if (metadata === null || typeof metadata !== "object") return false;
    const marker = (metadata as Record<string, unknown>).marker;
    if (marker === null || typeof marker !== "object") return false;
    return (marker as Record<string, unknown>).kind != null;
}

function cleanUserText(text: string): string {
    return removeSystemReminders(text).replaceAll(OMO_INTERNAL_INITIATOR_MARKER, "").trim();
}

export function isMeaningfulUserText(text: string): boolean {
    const cleaned = cleanUserText(text);
    return cleaned.length > 0 && !isSystemInjectedText(cleaned);
}

export function hasMeaningfulUserText(parts: unknown[]): boolean {
    for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        const candidate = part as Record<string, unknown>;
        if (candidate.type !== "text" || typeof candidate.text !== "string") continue;
        if (isMachineAuthoredPart(candidate)) continue;
        if (isMeaningfulUserText(candidate.text)) return true;
    }

    return false;
}

export function extractTexts(parts: unknown[], role: string): string[] {
    const texts: string[] = [];
    for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        const p = part as Record<string, unknown>;
        if (p.type !== "text" || typeof p.text !== "string") continue;
        if (isMachineAuthoredPart(p)) continue;
        // `hasMeaningfulUserText` evaluates cleaned text, so summaries clean user text too.
        const text = role === "user" ? cleanUserText(p.text) : p.text.trim();
        if (text.length === 0) continue;
        // An injected notice admitted beside real user text is machine control text, not user input.
        if (role === "user" && isSystemInjectedText(text)) continue;
        texts.push(text);
    }
    return texts;
}

/**
 * extractToolCallSummaries returns tool-call summaries prefixed with `TC:`. */
export function extractToolCallSummaries(parts: unknown[]): string[] {
    const summaries: string[] = [];
    for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        const p = part as Record<string, unknown>;
        if (p.type !== "tool" || typeof p.tool !== "string") continue;

        const state = p.state as Record<string, unknown> | null;
        if (!state || typeof state !== "object") continue;
        const input = state.input as Record<string, unknown> | null;
        const metadata = state.metadata as Record<string, unknown> | null;

        const description =
            (input && typeof input.description === "string" && input.description) ||
            (metadata && typeof metadata.description === "string" && metadata.description);
        if (description) {
            summaries.push(`TC: ${description}`);
            continue;
        }

        const toolName = p.tool as string;
        const keyArg = extractKeyArg(toolName, input);
        summaries.push(keyArg ? `TC: ${toolName}(${keyArg})` : `TC: ${toolName}`);
    }
    return summaries;
}

function extractKeyArg(_toolName: string, input: Record<string, unknown> | null): string | null {
    if (!input) return null;
    if (typeof input.filePath === "string") return truncateArg(input.filePath);
    if (typeof input.path === "string") return truncateArg(input.path);
    if (typeof input.pattern === "string") return truncateArg(input.pattern);
    if (typeof input.query === "string") return truncateArg(input.query);
    // Symbol tools
    if (typeof input.symbol === "string") return input.symbol;
    // Module tools
    if (typeof input.module === "string") return input.module;
    if (typeof input.action === "string") return input.action;
    return null;
}

function truncateArg(value: string, maxLen = 60): string {
    // Counting code points keeps a surrogate pair whole; `String.prototype.slice` on code units can split one.
    const codePoints = Array.from(value);
    if (codePoints.length <= maxLen) return value;
    return `${codePoints.slice(0, maxLen).join("")}…`;
}

export { estimateTokens, preloadTokenizer } from "../../shared/token-estimator";

export function normalizeText(text: string): string {
    // `\s` omits U+0085 NEXT LINE, which Unicode `White_Space` includes; the daemon's `split_whitespace` collapses it.
    return text.replace(/[\s\u0085]+/g, " ").trim();
}

export function compactRole(role: string): string {
    if (role === "assistant") return "A";
    if (role === "user") return "U";
    return role.slice(0, 1).toUpperCase() || "M";
}

export function formatBlock(block: ChunkBlock): string {
    const range =
        block.startOrdinal === block.endOrdinal
            ? `[${block.startOrdinal}]`
            : `[${block.startOrdinal}-${block.endOrdinal}]`;
    const commitSuffix =
        block.commitHashes.length > 0 ? ` commits: ${block.commitHashes.join(", ")}` : "";
    return `${range} ${block.role}:${commitSuffix} ${block.parts.join(" / ")}`;
}

function extractCommitHashes(text: string): string[] {
    const hashes: string[] = [];
    const seen = new Set<string>();
    for (const match of text.matchAll(createCommitHashExtractPattern())) {
        const hash = match[1]?.toLowerCase();
        if (!hash || seen.has(hash)) continue;
        seen.add(hash);
        hashes.push(hash);
        if (hashes.length >= MAX_COMMITS_PER_BLOCK) break;
    }
    return hashes;
}

export function compactTextForSummary(
    text: string,
    role: string,
): { text: string; commitHashes: string[] } {
    const commitHashes = role === "assistant" ? extractCommitHashes(text) : [];
    if (commitHashes.length === 0 || !COMMIT_VERB_PATTERN.test(text)) {
        return { text, commitHashes };
    }

    const withoutHashes = text
        .replace(createCommitHashExtractPattern(), removeHashKeepUnpairedBacktick)
        .replace(/\(\s*\)/g, "")
        .replace(/\s+,/g, ",")
        .replace(/,\s*,+/g, ", ")
        .replace(/\s{2,}/g, " ")
        .replace(/\s+([,.;:])/g, "$1")
        .trim();

    return {
        text: withoutHashes.length > 0 ? withoutHashes : text,
        commitHashes,
    };
}

/**
 * The extract pattern makes each backtick independently optional, so a hash inside a longer code
 * span (`git show abc1234`) matches with one backtick only. Dropping the whole match would leave
 * the span unbalanced; the lone backtick is put back.
 */
function removeHashKeepUnpairedBacktick(match: string): string {
    const opens = match.startsWith("`");
    const closes = match.endsWith("`");
    return opens === closes ? "" : "`";
}

export function mergeCommitHashes(existing: string[], next: string[]): string[] {
    if (next.length === 0) return existing;
    const merged = [...existing];
    for (const hash of next) {
        // The length check precedes `push`, so `merged` never exceeds `MAX_COMMITS_PER_BLOCK`.
        if (merged.length >= MAX_COMMITS_PER_BLOCK) break;
        if (merged.includes(hash)) continue;
        merged.push(hash);
    }
    return merged;
}
