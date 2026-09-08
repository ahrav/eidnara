import { COMMIT_VERB_PATTERN, createCommitHashExtractPattern } from "../../shared/commit-detection";
import { removeSystemInjections } from "../../shared/system-directive";
import { stripTagPrefix } from "./tag-content-primitives";

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

export const MAX_COMMITS_PER_BLOCK = 5;

export function isTruthyFlag(value: unknown): boolean {
    return value === true || value === 1 || value === "true";
}

export function isMachineAuthoredPart(part: Record<string, unknown>): boolean {
    if (
        isTruthyFlag(part.synthetic) ||
        isTruthyFlag(part.syntheticTodoMarker) ||
        isTruthyFlag(part.ignored)
    ) {
        return true;
    }
    const metadata = part.metadata;
    if (metadata === null || typeof metadata !== "object") return false;
    const marker = (metadata as Record<string, unknown>).marker;
    if (marker === null || typeof marker !== "object") return false;
    return (marker as Record<string, unknown>).kind != null;
}

// `String#trim` leaves U+0085 (NEXT LINE) in place; `normalizeText` already treats it as whitespace.
function trimText(text: string): string {
    return text.replace(/^[\s\u0085]+|[\s\u0085]+$/g, "");
}

// Remove system injections before stripping tag prefixes because removal can expose a leading tag.
function cleanUserText(text: string): string {
    return trimText(stripTagPrefix(removeSystemInjections(text)));
}

export function isMeaningfulUserText(text: string): boolean {
    return cleanUserText(text).length > 0;
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
        const text = role === "user" ? cleanUserText(p.text) : trimText(p.text);
        if (text.length === 0) continue;
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
        if (p.type !== "tool") continue;
        const toolName = resolveToolName(p);
        if (toolName === null) continue;
        // A synthetic tool part is the daemon's own bookkeeping, not a call the model made.
        if (isMachineAuthoredPart(p)) continue;

        const state = asRecord(p.state);
        const input = asRecord(state?.input) ?? asRecord(p.input) ?? asRecord(p.args);
        const metadata = asRecord(state?.metadata);

        const description =
            (input && typeof input.description === "string" && input.description) ||
            (metadata && typeof metadata.description === "string" && metadata.description);
        if (description) {
            summaries.push(`TC: ${description}`);
            continue;
        }

        const keyArg = extractKeyArg(toolName, input);
        summaries.push(keyArg ? `TC: ${toolName}(${keyArg})` : `TC: ${toolName}`);
    }
    return summaries;
}

function resolveToolName(part: Record<string, unknown>): string | null {
    for (const key of ["tool", "toolName", "name"] as const) {
        const value = part[key];
        if (typeof value === "string" && value.length > 0) return value;
    }
    return null;
}

function asRecord(value: unknown): Record<string, unknown> | null {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, unknown>)
        : null;
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
    // The first code point, not the first code unit: `slice(0, 1)` on an astral initial yields a lone surrogate.
    const initial = role.codePointAt(0);
    return initial === undefined ? "M" : String.fromCodePoint(initial).toUpperCase();
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

function extractCommitHashes(text: string, recorded: ReadonlySet<string>): string[] {
    const hashes: string[] = [];
    const capacity = MAX_COMMITS_PER_BLOCK - recorded.size;
    if (capacity <= 0) return hashes;
    const seen = new Set<string>();
    for (const match of text.matchAll(createCommitHashExtractPattern())) {
        const hash = match[1]?.toLowerCase();
        if (!hash || seen.has(hash) || recorded.has(hash)) continue;
        seen.add(hash);
        hashes.push(hash);
        if (hashes.length >= capacity) break;
    }
    return hashes;
}

/**
 * `recordedHashes` are the hashes the enclosing block already holds. A hash already recorded does
 * not spend a capacity slot but is still removed from the text, so a repeat mention beside a new
 * hash lets the new one through. Hashes past the block cap stay in the text.
 */
export function compactTextForSummary(
    text: string,
    role: string,
    recordedHashes: readonly string[] = [],
): { text: string; commitHashes: string[] } {
    if (role !== "assistant") return { text, commitHashes: [] };
    const recorded = new Set(recordedHashes.map((hash) => hash.toLowerCase()));
    const commitHashes = extractCommitHashes(text, recorded);
    if (!COMMIT_VERB_PATTERN.test(text)) return { text, commitHashes };

    const removable = new Set([...recorded, ...commitHashes]);
    let removed = 0;
    const withoutHashes = text
        .replace(createCommitHashExtractPattern(), (match, hash: string) => {
            if (!removable.has(hash.toLowerCase())) return match;
            removed += 1;
            return removeHashKeepUnpairedBacktick(match) + REMOVED_HASH;
        })
        .replace(EMPTIED_PARENS_OR_MARKER, "")
        .replace(/\s+,/g, ",")
        .replace(/,\s*,+/g, ", ")
        .replace(/\s{2,}/g, " ")
        .replace(/\s+([,.;:])/g, "$1")
        .trim();
    if (removed === 0) return { text, commitHashes };

    return {
        text: withoutHashes.length > 0 ? withoutHashes : text,
        commitHashes,
    };
}

// The marker lets cleanup remove only parentheses emptied by hash removal; `foo()` elsewhere is kept.
const REMOVED_HASH = "\ue000";
const EMPTIED_PARENS_OR_MARKER = /\(\s*\ue000(?:\s*,\s*\ue000)*\s*\)|\ue000/g;

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
