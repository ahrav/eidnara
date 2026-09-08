import type { ContextLimitProvenance } from "../../shared/context-limit-provenance";

/**
 *
 *      turn fits.
 *
 * We copy OpenCode's BSD-licensed logic to avoid coupling the plugin to OpenCode versions.
 *
 * References:
 *     https://github.com/sst/opencode/blob/main/packages/opencode/src/provider/error.ts
 *     https://github.com/earendil-works/pi-mono/blob/main/packages/ai/src/utils/overflow.ts
 */

/**
 * emerge.
 *
 * Provider error text is attacker-controlled; bound gaps between literals to prevent superlinear backtracking.
 */
export const OVERFLOW_PATTERNS: ReadonlyArray<RegExp> = [
    /prompt is too long/i, // Anthropic
    /input is too long for requested model/i, // Amazon Bedrock
    /exceeds the context window/i, // OpenAI (Completions + Responses API)
    /input token count.{0,80}?exceeds the maximum/i, // Google Gemini
    /maximum prompt length is \d+/i, // xAI (Grok)
    /reduce the length of the messages/i, // Groq
    /maximum context length is \d+ tokens/i, // OpenRouter, DeepSeek, vLLM
    /maximum model length is \d+/i, // vLLM
    /exceeds the limit of \d+/i, // GitHub Copilot
    /exceeds the available context size/i, // llama.cpp server
    /greater than the context length/i, // LM Studio
    /context window exceeds limit/i, // MiniMax
    /exceeded model token limit/i, // Kimi For Coding, Moonshot
    /context[_ ]length[_ ]exceeded/i, // Generic fallback
    /request entity too large/i, // HTTP 413
    /context length is only \d+ tokens/i, // vLLM
    /input length.{0,80}?exceeds.{0,80}?context length/i, // vLLM
    /prompt too long; exceeded (?:max )?context length/i, // Ollama explicit overflow
    /too large for model with \d+ maximum context length/i, // Mistral
    /model_context_window_exceeded/i, // z.ai non-standard finish_reason
    /context size has been exceeded/i, // Lemonade / llama-cpp wrappers
];

/**
 * Each pattern's first capture group is the numeric token limit.
 *
 * The caller can still use the overflow signal when no numeric limit is available.
 */
interface LimitExtractionPattern {
    pattern: RegExp;
    provenance: ContextLimitProvenance;
}

const LIMIT_EXTRACTION_PATTERNS: ReadonlyArray<LimitExtractionPattern> = [
    { pattern: /maximum prompt length is (\d+)/i, provenance: "prompt_only" }, // xAI
    {
        pattern: /maximum context length is (\d+) tokens?/i,
        provenance: "combined",
    }, // OpenAI / OpenRouter / DeepSeek / vLLM
    { pattern: /maximum model length is (\d+)/i, provenance: "combined" }, // vLLM
    { pattern: /context length is only (\d+) tokens?/i, provenance: "combined" }, // vLLM
    { pattern: /exceeds the limit of (\d+)/i, provenance: "unknown" }, // GitHub Copilot
    {
        pattern: /too large for model with (\d+) maximum context length/i,
        provenance: "combined",
    }, // Mistral
    // Limit the gap to 40 non-digits so the capture selects the first four-or-more-digit token count.
    // An unbounded `.*` gap can skip the intended token count.
    { pattern: /context size[^0-9]{0,40}(\d{4,})\s*tokens?/i, provenance: "combined" }, // llama.cpp variants
    // Capture the context limit rather than the prompt size.
    // The explicit pattern prevents the fallback from extracting the prompt size.
    { pattern: /exceeds? the context length of (\d+)/i, provenance: "combined" }, // vLLM overflow
    {
        pattern: />\s*(\d+)\s*(?:tokens?\s*)?(?:maximum|max|limit)\b/i,
        provenance: "prompt_only",
    }, // Anthropic reports the accepted input ceiling, not input plus output.
    // The generic fallback's greedy gap skips a limit stated before `context` and captures a later request size.
    { pattern: /max(?:imum)?\s+(\d+)\s+context\b/i, provenance: "unknown" },
    { pattern: /max(?:imum)?.{0,80}context.{0,40}?(\d+)/i, provenance: "unknown" }, // generic fallback
];

/**
 * Reject limits below 1024 to avoid unrelated numeric fields such as error codes.
 * */
const MIN_PLAUSIBLE_LIMIT = 1024;
/**
 * Reject larger values to avoid matching token-count fields instead of limits. */
const MAX_PLAUSIBLE_LIMIT = 10_000_000;
/** Limits regex input to the leading characters, bounding cost for attacker-controlled bodies. */
export const MAX_SCAN_CHARS = 4096;

export interface ReportedContextLimit {
    value: number;
    provenance: ContextLimitProvenance;
}

export interface OverflowDetection {
    /* */
    isOverflow: boolean;
    /* */
    reportedLimit?: number;
    /** The provenance identifies whether the limit covers the prompt alone or the combined context window. */
    reportedLimitProvenance?: ContextLimitProvenance;
    /* */
    matchedPattern?: string;
}

const MAX_ERROR_TEXT_DEPTH = 6;
const MAX_ERROR_TEXTS = 12;
const ERROR_TEXT_KEYS = ["message", "responseBody"] as const;
const NESTED_ERROR_KEYS = ["error", "data", "cause", "body"] as const;

/**
 * `JSON.stringify` serializes a whole decoded body before any cap applies, so this walk stops emitting at `limit`.
 * Output is JSON-shaped for readability in logs, not guaranteed to parse.
 */
function boundedStringify(value: unknown, limit: number): string {
    let out = "";
    let truncated = false;
    const seen = new WeakSet<object>();
    const emit = (chunk: string): boolean => {
        const room = limit - out.length;
        if (chunk.length > room) {
            out += chunk.slice(0, room);
            truncated = true;
            return false;
        }
        out += chunk;
        return true;
    };
    const walk = (current: unknown, depth: number): boolean => {
        if (current === null || current === undefined) return emit("null");
        switch (typeof current) {
            case "string":
                return emit(JSON.stringify(current.slice(0, limit)));
            case "number":
            case "boolean":
            case "bigint":
                return emit(String(current));
            case "object":
                break;
            default:
                return emit(`"[${typeof current}]"`);
        }
        const obj = current as object;
        if (seen.has(obj)) return emit('"[cycle]"');
        if (depth >= MAX_ERROR_TEXT_DEPTH) return emit('"[depth]"');
        seen.add(obj);
        if (Array.isArray(obj)) {
            if (!emit("[")) return false;
            for (let i = 0; i < obj.length; i += 1) {
                if (i > 0 && !emit(",")) return false;
                if (!walk(obj[i], depth + 1)) return false;
            }
            return emit("]");
        }
        if (!emit("{")) return false;
        let first = true;
        // `for...in` defers each property's value read until its iteration, so `emit` can stop traversal before later reads.
        for (const key in obj) {
            if (!Object.hasOwn(obj, key)) continue;
            const entry = (obj as Record<string, unknown>)[key];
            if (entry === undefined || typeof entry === "function" || typeof entry === "symbol") {
                continue;
            }
            if (!first && !emit(",")) return false;
            first = false;
            if (!emit(`${JSON.stringify(key)}:`)) return false;
            if (!walk(entry, depth + 1)) return false;
        }
        return emit("}");
    };
    walk(value, 0);
    return truncated ? `${out}…` : out;
}

/**
 * OpenCode events deliver errors as strings, Error instances, or objects with `message`.
 */
export function extractErrorMessage(error: unknown): string {
    if (!error) return "";
    if (typeof error === "string") return error;
    // Checking `error.error.message` first preserves nested provider error messages.
    if (typeof error === "object") {
        const obj = error as Record<string, unknown>;
        const nested = obj.error as Record<string, unknown> | undefined;
        if (nested && typeof nested.message === "string" && nested.message.length > 0) {
            return nested.message;
        }
    }
    if (error instanceof Error) return error.message;
    if (typeof error === "object") {
        const obj = error as Record<string, unknown>;
        if (typeof obj.message === "string") return obj.message;
        if (typeof obj.responseBody === "string") return obj.responseBody;
        return boundedStringify(error, MAX_SCAN_CHARS);
    }
    return String(error);
}

/**
 * Provider text can reside in `responseBody`, `cause`, or a nested `error` object under a generic wrapper `message`.
 * The list starts with `extractErrorMessage` so the wrapper's primary text is scanned first.
 */
function collectErrorTexts(error: unknown): string[] {
    const texts: string[] = [];
    const primary = extractErrorMessage(error);
    if (primary) texts.push(primary);

    const seen = new WeakSet<object>();
    const visit = (value: unknown, depth: number): void => {
        if (texts.length >= MAX_ERROR_TEXTS) return;
        if (typeof value === "string") {
            if (value && !texts.includes(value)) texts.push(value);
            return;
        }
        if (!value || typeof value !== "object") return;
        if (seen.has(value) || depth >= MAX_ERROR_TEXT_DEPTH) return;
        seen.add(value);
        const obj = value as Record<string, unknown>;
        for (const key of ERROR_TEXT_KEYS) {
            const text = obj[key];
            if (typeof text === "string" && text && !texts.includes(text)) texts.push(text);
        }
        for (const key of NESTED_ERROR_KEYS) {
            visit(obj[key], depth + 1);
        }
    };
    visit(error, 0);
    return texts;
}

function matchOverflowPattern(message: string): RegExp | undefined {
    for (const pattern of OVERFLOW_PATTERNS) {
        if (pattern.test(message)) return pattern;
    }
    return undefined;
}

function hasStatus413(message: string): boolean {
    return /\b413\b/.test(message) && /(entity|payload|context|prompt)/i.test(message);
}

export function detectOverflow(error: unknown): OverflowDetection {
    for (const candidate of collectErrorTexts(error)) {
        const message = candidate.slice(0, MAX_SCAN_CHARS);
        const matched = matchOverflowPattern(message);
        if (!matched && !hasStatus413(message)) continue;

        const reportedLimit = parseReportedLimit(message);
        return {
            isOverflow: true,
            reportedLimit: reportedLimit?.value,
            reportedLimitProvenance: reportedLimit?.provenance,
            matchedPattern: matched?.source,
        };
    }
    return { isOverflow: false };
}

/**
 */
export function parseReportedLimit(message: string): ReportedContextLimit | undefined {
    if (!message) return undefined;
    const scanned = message.slice(0, MAX_SCAN_CHARS);
    for (const { pattern, provenance } of LIMIT_EXTRACTION_PATTERNS) {
        const match = scanned.match(pattern);
        if (!match) continue;
        const raw = match[1];
        if (!raw) continue;
        const value = Number.parseInt(raw, 10);
        if (!Number.isFinite(value)) continue;
        if (value < MIN_PLAUSIBLE_LIMIT || value > MAX_PLAUSIBLE_LIMIT) continue;
        return { value, provenance };
    }
    return undefined;
}
