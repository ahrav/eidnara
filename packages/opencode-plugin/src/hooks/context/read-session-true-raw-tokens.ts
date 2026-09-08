import { isRecord } from "../../shared/record-type-guard";
import { stableStringify } from "../../shared/stable-json";
import { estimateTokens } from "./read-session-formatting";
import type { RawMessage } from "./read-session-raw";

export interface TrueRawTokenBreakdown {
    text: number;
    reasoning: number;
    toolInput: number;
    toolOutput: number;
    image: number;
    other: number;
    total: number;
}

export type ProviderShapeVersion = "opencode-v1" | "pi-folded-v1";

export interface TrueRawEstimateOptions {
    providerShapeVersion: ProviderShapeVersion;
    imageTokenHeuristic?: (part: unknown) => number;
}

/**
 * Rules for part types that differ by harness decoder.
 * Types outside `toolTypes` are never tool signals, even when they carry tool-like fields.
 * `skippedTypes` are bookkeeping parts the decoder discards.
 * `honorsIgnoredText` drops `text` parts flagged `ignored: true`.
 */
interface ProviderPartRules {
    readonly toolTypes: ReadonlySet<string>;
    readonly skippedTypes: ReadonlySet<string>;
    readonly honorsIgnoredText: boolean;
}

const GENERIC_TOOL_TYPES = ["tool_use", "tool_result", "tool-invocation"] as const;

const PROVIDER_PART_RULES: Record<ProviderShapeVersion, ProviderPartRules> = {
    "opencode-v1": {
        toolTypes: new Set([...GENERIC_TOOL_TYPES, "tool"]),
        skippedTypes: new Set(["snapshot", "patch", "agent", "retry", "compaction"]),
        honorsIgnoredText: true,
    },
    "pi-folded-v1": {
        toolTypes: new Set([...GENERIC_TOOL_TYPES, "toolCall"]),
        skippedTypes: new Set(),
        honorsIgnoredText: false,
    },
};

function partRulesFor(providerShapeVersion: ProviderShapeVersion): ProviderPartRules {
    return PROVIDER_PART_RULES[providerShapeVersion];
}

export interface TrueRawTokenIndex {
    readonly sessionId: string;
    readonly providerShapeVersion: string;
    readonly rawMessageCount: number;
    tokenForOrdinal(ordinal: number): number;
    messageIdAtOrdinal(ordinal: number): string | null;
    suffixTokensFromOrdinal(ordinal: number): number;
    rangeTokens(startInclusive: number, endExclusive: number): number;
    findSuffixStartForTokens(tokens: number): number;
    findHeadEndForCap(startInclusive: number, endExclusive: number, capTokens: number): number;
}

export interface ToolArc {
    callId: string;
    invOrdinal: number;
    resOrdinal: number | null;
}

/** `completedToolArcCrossesBoundary` returns true when a tail beginning at `boundary` retains a completed result without its call. */
export function completedToolArcCrossesBoundary(
    invOrdinal: number,
    resOrdinal: number,
    boundary: number,
): boolean {
    return invOrdinal < boundary && boundary <= resOrdinal;
}

export interface TrueRawTokenIndexBuildOptions extends TrueRawEstimateOptions {
    cacheNamespace: string;
    /**
     * A non-null return value supplies the message's total token count.
     * A non-null return value replaces live tokenization for that message.
     * The protected-tail boundary uses stored totals instead of live-tokenizing message parts.
     * A null return value falls back to live tokenization for that message.
     */
    storedTotalForMessage?: (message: RawMessage) => number | null;
    /**
     * Tail-only callers must set `absoluteMessageCount` to the absolute session count when `messages` retains absolute ordinals.
     * `messages` may contain only the tail while retaining absolute ordinals.
     * `absoluteMessageCount` sizes the prefix and suffix structures.
     * Messages outside the supplied slice contribute zero tokens.
     * Protected-tail queries never read below the eligible offset.
     * The boundary can resolve from the eligible tail without reading the whole session.
     *
     * `absoluteMessageCount` defaults to `messages.length` for whole-session callers.
     * index-positional fill.
     */
    absoluteMessageCount?: number;
}

interface CachedMessageEstimate {
    breakdown: TrueRawTokenBreakdown;
    keyEstimateBytes: number;
}

interface ToolSignal {
    callId: string;
    toolName: string;
    hasInput: boolean;
    hasOutput: boolean;
    /** Provider-executed tools run on the provider's side, so they never form an in-flight local arc. */
    providerExecuted: boolean;
    /** Error polarity of the result, which the decoders preserve as a distinct output kind. */
    isError: boolean;
    inputText: string;
    outputText: string;
    /** Image and file blocks inside a tool result, counted through the image heuristic instead of as text. */
    outputMedia: readonly Record<string, unknown>[];
    /** `state.metadata.description`, which the tool-call summaries display when the input carries no description. */
    metadataDescription: string;
}

interface ToolResultContent {
    text: string;
    media: Record<string, unknown>[];
}

const MAX_MESSAGE_CACHE_ENTRIES = 100_000;
const MAX_MESSAGE_CACHE_KEY_BYTES = 64 * 1024 * 1024;
const FNV1A_32_OFFSET = 0x811c9dc5;
const FNV1A_32_PRIME = 0x01000193;
const messageEstimateCache = new Map<string, CachedMessageEstimate>();
let messageEstimateCacheBytes = 0;
const imageHeuristicIds = new WeakMap<(part: unknown) => number, number>();
let nextImageHeuristicId = 1;

const EMPTY_BREAKDOWN: TrueRawTokenBreakdown = {
    text: 0,
    reasoning: 0,
    toolInput: 0,
    toolOutput: 0,
    image: 0,
    other: 0,
    total: 0,
};

function addBreakdown(
    target: TrueRawTokenBreakdown,
    kind: keyof TrueRawTokenBreakdown,
    value: number,
): void {
    if (kind === "total") return;
    const safeValue = Number.isFinite(value) && value > 0 ? Math.round(value) : 0;
    target[kind] += safeValue;
    target.total += safeValue;
}

function estimateStructured(value: unknown): number {
    if (typeof value === "string") return estimateTokens(value);
    if (value === undefined || value === null) return 0;
    return estimateTokens(stableStringify(value));
}

function firstStringField(
    record: Record<string, unknown>,
    fields: readonly string[],
): string | null {
    for (const field of fields) {
        const value = record[field];
        if (typeof value === "string" && value.length > 0) return value;
    }
    return null;
}

/** An empty string is a present value here: an empty text block is empty output, not a serialized record. */
function firstStringFieldAllowEmpty(
    record: Record<string, unknown>,
    fields: readonly string[],
): string | null {
    for (const field of fields) {
        const value = record[field];
        if (typeof value === "string") return value;
    }
    return null;
}

function stringValue(value: unknown): string {
    if (typeof value === "string") return value;
    if (value === undefined || value === null) return "";
    return stableStringify(value);
}

/**
 * Content blocks dispatch on their explicit type; MIME presence matters only when untyped.
 * OpenCode attachments are media whenever they carry a MIME field, whatever their type.
 */
type ResultBlockOrigin = "content" | "attachment";

function isMediaResultBlock(entry: Record<string, unknown>, origin: ResultBlockOrigin): boolean {
    const type = partType(entry);
    const hasMime = hasOwn(entry, "mime") || hasOwn(entry, "mimeType");
    if (origin === "attachment") {
        return type === "image" || type === "file" || hasMime || looksImageLike(entry);
    }
    if (type === "image" || type === "file") return true;
    if (type === "text") return false;
    if (type.length > 0) return looksImageLike(entry);
    return hasMime || looksImageLike(entry);
}

function mergeToolResultContent(a: ToolResultContent, b: ToolResultContent): ToolResultContent {
    return {
        text: [a.text, b.text].filter((text) => text.length > 0).join("\n"),
        media: [...a.media, ...b.media],
    };
}

/**
 * Image and file blocks are separated from text so a base64 payload is never tokenized as text.
 * A block with an explicit type other than `text` is opaque and contributes its whole serialized form.
 */
function toolResultContent(
    content: unknown,
    origin: ResultBlockOrigin = "content",
): ToolResultContent {
    if (typeof content === "string") return { text: content, media: [] };
    if (Array.isArray(content)) {
        const pieces: string[] = [];
        const media: Record<string, unknown>[] = [];
        for (const entry of content) {
            if (typeof entry === "string") {
                pieces.push(entry);
            } else if (isRecord(entry)) {
                if (isMediaResultBlock(entry, origin)) {
                    media.push(entry);
                    continue;
                }
                const type = partType(entry);
                const text =
                    type === "text" || type.length === 0
                        ? firstStringFieldAllowEmpty(entry, ["text", "content", "value"])
                        : null;
                pieces.push(text ?? stableStringify(entry));
            } else if (entry !== null && entry !== undefined) {
                pieces.push(String(entry));
            }
        }
        return { text: pieces.join("\n"), media };
    }
    if (isRecord(content) && isMediaResultBlock(content, origin)) {
        return { text: "", media: [content] };
    }
    return { text: stringValue(content), media: [] };
}

function looksImageLike(part: Record<string, unknown>): boolean {
    const type = typeof part.type === "string" ? part.type.toLowerCase() : "";
    const mime = typeof part.mime === "string" ? part.mime.toLowerCase() : "";
    const mediaType = typeof part.mediaType === "string" ? part.mediaType.toLowerCase() : "";
    return (
        type.includes("image") ||
        mime.startsWith("image/") ||
        mediaType.startsWith("image/") ||
        part.image_url !== undefined ||
        part.imageUrl !== undefined ||
        part.image !== undefined
    );
}

function defaultImageTokenHeuristic(part: unknown): number {
    if (isRecord(part)) {
        const width = part.width;
        const height = part.height;
        if (typeof width === "number" && typeof height === "number" && width > 0 && height > 0) {
            return Math.max(256, Math.min(4096, Math.ceil((width * height) / 750)));
        }
    }
    return 1024;
}

function partType(part: Record<string, unknown>): string {
    return typeof part.type === "string" ? part.type : "";
}

function hasOwn(record: Record<string, unknown>, key: string): boolean {
    return Object.hasOwn(record, key);
}

/** Presence, not value, decides a match: an own `null` or `undefined` field is a present field. */
function firstOwnKey(record: Record<string, unknown>, keys: readonly string[]): string | null {
    for (const key of keys) {
        if (hasOwn(record, key)) return key;
    }
    return null;
}

function recursiveByteLength(value: unknown): number {
    if (value === null || value === undefined) return 0;
    if (typeof value === "string") return value.length;
    if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
        return String(value).length;
    }
    if (Array.isArray(value)) {
        return value.reduce((sum, item) => sum + recursiveByteLength(item), value.length);
    }
    if (isRecord(value)) {
        let total = Object.keys(value).length;
        for (const [key, child] of Object.entries(value)) {
            total += key.length + recursiveByteLength(child);
        }
        return total;
    }
    return String(value).length;
}

function updateFnv1a32(hash: number, text: string): number {
    let next = hash;
    for (let index = 0; index < text.length; index += 1) {
        next ^= text.charCodeAt(index);
        next = Math.imul(next, FNV1A_32_PRIME) >>> 0;
    }
    return next;
}

function contentStringsHash(fields: readonly string[]): string {
    let hash = FNV1A_32_OFFSET;
    for (const field of fields) {
        hash = updateFnv1a32(hash, `${field.length}:`);
        hash = updateFnv1a32(hash, field);
        hash = updateFnv1a32(hash, "\0");
    }
    return hash.toString(16).padStart(8, "0");
}

function rawPartVersion(part: Record<string, unknown>): unknown {
    return (
        part.__eidnaraPartUpdatedAt ??
        part.updated_at ??
        part.updatedAt ??
        part.version ??
        part.revision ??
        ""
    );
}

const TOOL_CALL_ID_FIELDS = [
    "callID",
    "callId",
    "toolCallId",
    "tool_call_id",
    "tool_use_id",
    "toolUseId",
    "id",
] as const;

/** Pi `toolCall` blocks name the call in `id`. */
const PI_TOOL_CALL_ID_FIELDS = ["id", "callId", "toolCallId"] as const;

function callIdFromPart(part: Record<string, unknown>): string {
    const fields = partType(part) === "toolCall" ? PI_TOOL_CALL_ID_FIELDS : TOOL_CALL_ID_FIELDS;
    const direct = firstStringField(part, fields);
    if (direct) return direct;
    const state = isRecord(part.state) ? part.state : null;
    return state ? (firstStringField(state, fields) ?? "") : "";
}

function toolNameFromPart(part: Record<string, unknown>): string {
    return firstStringField(part, ["tool", "toolName", "name"]) ?? "";
}

/** OpenCode persists the flag either at the top level or under `metadata`. */
function providerExecutedFromPart(part: Record<string, unknown>): boolean {
    if (part.providerExecuted === true) return true;
    const metadata = isRecord(part.metadata) ? part.metadata : null;
    return metadata !== null && metadata.providerExecuted === true;
}

const TERMINAL_TOOL_STATUSES = new Set(["completed", "error"]);

/** Only a terminal `status` completes an OpenCode tool. A stored `output` on a `running` tool is partial streamed output, not a result. */
function toolStatusIsTerminal(
    part: Record<string, unknown>,
    state: Record<string, unknown> | null,
): boolean {
    const status =
        (state ? firstStringField(state, ["status"]) : null) ?? firstStringField(part, ["status"]);
    return status !== null && TERMINAL_TOOL_STATUSES.has(status);
}

function emptyToolResultContent(): ToolResultContent {
    return { text: "", media: [] };
}

/** A present `state.attachments` wins even when it is not an array; the decoder never falls back to the top level then. */
function toolAttachments(
    part: Record<string, unknown>,
    state: Record<string, unknown> | null,
): ToolResultContent {
    const attachments =
        state && hasOwn(state, "attachments") ? state.attachments : part.attachments;
    if (!Array.isArray(attachments)) return emptyToolResultContent();
    return toolResultContent(attachments.filter(isRecord), "attachment");
}

function metadataDescriptionFromState(state: Record<string, unknown> | null): string {
    const metadata = state && isRecord(state.metadata) ? state.metadata : null;
    return metadata ? (firstStringField(metadata, ["description"]) ?? "") : "";
}

function resultIsError(part: Record<string, unknown>): boolean {
    return part.isError === true || part.is_error === true;
}

/**
 * A folded Pi tool result is a part with `role: "toolResult"` and no `type`.
 * Harnesses that synthesize a call identity for idless parts: OpenCode `tool` and Pi `toolCall`.
 */
function toolPartType(part: Record<string, unknown>): string {
    const type = partType(part);
    if (type.length === 0 && part.role === "toolResult") return "tool_result";
    return type;
}

const SYNTHESIZED_ID_TOOL_TYPES = new Set(["tool", "toolCall"]);

/** The Pi decoder names a folded result with no `toolCallId` after the generic tool. */
const FOLDED_RESULT_DEFAULT_CALL_ID = "tool";

function toolSignalFromPart(part: unknown, rules: ProviderPartRules): ToolSignal | null {
    if (!isRecord(part)) return null;
    const type = toolPartType(part);
    if (!rules.toolTypes.has(type)) return null;
    const state = isRecord(part.state) ? part.state : null;
    let callId = callIdFromPart(part);
    if (!callId && part.role === "toolResult") callId = FOLDED_RESULT_DEFAULT_CALL_ID;
    if (!callId && !SYNTHESIZED_ID_TOOL_TYPES.has(type)) return null;
    const toolName = toolNameFromPart(part);

    if (type === "tool") {
        const inputOwner = state && hasOwn(state, "input") ? state : part;
        const inputKey = firstOwnKey(inputOwner, ["input", "args"]);
        // Non-string `output` or `error` values produce empty text; attachments decode separately.
        const outputOwner =
            state && firstOwnKey(state, ["output", "error"]) !== null ? state : part;
        const outputKey = firstOwnKey(outputOwner, ["output", "error"]);
        const outputValue = outputKey ? outputOwner[outputKey] : undefined;
        const status =
            (state ? firstStringField(state, ["status"]) : null) ??
            firstStringField(part, ["status"]);
        const output = mergeToolResultContent(
            { text: typeof outputValue === "string" ? outputValue : "", media: [] },
            toolAttachments(part, state),
        );
        return {
            callId,
            toolName,
            hasInput: true,
            hasOutput: toolStatusIsTerminal(part, state),
            providerExecuted: providerExecutedFromPart(part),
            isError: status === "error" || outputKey === "error",
            inputText: inputKey ? stringValue(inputOwner[inputKey]) : "",
            outputText: output.text,
            outputMedia: output.media,
            metadataDescription: metadataDescriptionFromState(state),
        };
    }

    if (type === "tool-invocation") {
        const argsKey = firstOwnKey(part, ["args", "input"]);
        const outputKey = firstOwnKey(part, ["result", "output"]);
        const output = outputKey ? toolResultContent(part[outputKey]) : emptyToolResultContent();
        return {
            callId,
            toolName,
            hasInput: true,
            hasOutput: outputKey !== null,
            providerExecuted: false,
            isError: resultIsError(part) || part.state === "error",
            inputText: argsKey ? stringValue(part[argsKey]) : "",
            outputText: output.text,
            outputMedia: output.media,
            metadataDescription: "",
        };
    }

    if (type === "tool_use" || type === "toolCall") {
        const inputKey = firstOwnKey(
            part,
            type === "toolCall" ? ["arguments", "input"] : ["input"],
        );
        return {
            callId,
            toolName,
            hasInput: true,
            hasOutput: false,
            providerExecuted: false,
            isError: false,
            inputText: inputKey ? stringValue(part[inputKey]) : "",
            outputText: "",
            outputMedia: [],
            metadataDescription: "",
        };
    }

    if (type === "tool_result") {
        const contentKey = firstOwnKey(part, ["content", "output", "result"]);
        const output = contentKey ? toolResultContent(part[contentKey]) : emptyToolResultContent();
        return {
            callId,
            toolName,
            hasInput: false,
            hasOutput: true,
            providerExecuted: false,
            isError: resultIsError(part),
            inputText: "",
            outputText: output.text,
            outputMedia: output.media,
            metadataDescription: "",
        };
    }

    return null;
}

/** Primitive and array parts are tokenized as their serialized form, so they fingerprint the same way. */
function nonRecordPartFingerprint(part: unknown): string {
    return `${typeof part}:h${contentStringsHash([stringValue(part)])}`;
}

function partCheapFingerprint(part: unknown): string {
    if (!isRecord(part)) return nonRecordPartFingerprint(part);
    const version = rawPartVersion(part);
    const type = typeof part.type === "string" ? part.type : "";
    const byteLength = recursiveByteLength(part);
    if (version !== "") return `${type}:${String(version)}:${byteLength}`;
    return `${type}:h${contentStringsHash([stableStringify(part)])}:${byteLength}`;
}

/**
 * The estimate cache is process-local, so a heuristic's function identity distinguishes its
 * entries. Callers that want cache hits across builds must pass the same function reference.
 */
function imageHeuristicCacheKey(heuristic: TrueRawEstimateOptions["imageTokenHeuristic"]): string {
    if (!heuristic) return "image:default";
    let id = imageHeuristicIds.get(heuristic);
    if (id === undefined) {
        id = nextImageHeuristicId;
        nextImageHeuristicId += 1;
        imageHeuristicIds.set(heuristic, id);
    }
    return `image:${id}`;
}

/** `invalidateTrueRawTokenCache` matches session and message IDs only as `\0`-delimited segments. */
function messageCacheKey(
    sessionId: string,
    message: RawMessage,
    options: TrueRawTokenIndexBuildOptions,
): string {
    const cheapFingerprint = message.parts.map(partCheapFingerprint).join("|");
    return [
        options.cacheNamespace,
        sessionId,
        options.providerShapeVersion,
        imageHeuristicCacheKey(options.imageTokenHeuristic),
        message.id || `ordinal:${message.ordinal}`,
        message.role,
        message.parts.length,
        cheapFingerprint,
    ].join("\0");
}

function setCachedEstimate(key: string, breakdown: TrueRawTokenBreakdown): void {
    const keyEstimateBytes = key.length * 2 + 64;
    const existing = messageEstimateCache.get(key);
    if (existing) messageEstimateCacheBytes -= existing.keyEstimateBytes;
    messageEstimateCache.set(key, { breakdown, keyEstimateBytes });
    messageEstimateCacheBytes += keyEstimateBytes;
    while (
        messageEstimateCache.size > MAX_MESSAGE_CACHE_ENTRIES ||
        messageEstimateCacheBytes > MAX_MESSAGE_CACHE_KEY_BYTES
    ) {
        const first = messageEstimateCache.keys().next().value;
        if (typeof first !== "string") break;
        const removed = messageEstimateCache.get(first);
        if (removed) messageEstimateCacheBytes -= removed.keyEstimateBytes;
        messageEstimateCache.delete(first);
    }
}

function cloneBreakdown(value: TrueRawTokenBreakdown): TrueRawTokenBreakdown {
    return { ...value };
}

type NonToolPartContent =
    | { kind: "skip" }
    | { kind: "text"; text: string }
    | { kind: "reasoning"; text: string }
    | { kind: "image"; altText: string | null }
    | { kind: "structured" };

/**
 * The opaque payload of a redacted reasoning block.
 * Anthropic stores it in `data`; Pi in `thinkingSignature` with `redacted: true`.
 * OpenCode stores it in a string `redacted` field at the top level or under `metadata`.
 */
function redactedReasoningData(part: Record<string, unknown>): string | null {
    const direct = firstStringField(part, ["data"]);
    if (direct) return direct;
    if (typeof part.redacted === "string" && part.redacted.length > 0) return part.redacted;
    const metadata = isRecord(part.metadata) ? part.metadata : null;
    const fromMetadata = metadata ? firstStringField(metadata, ["redacted"]) : null;
    if (fromMetadata) return fromMetadata;
    if (part.redacted === true || partType(part) === "redacted_thinking") {
        return firstStringField(part, ["thinkingSignature"]);
    }
    return null;
}

/**
 * Invariant: `estimateNonToolPart` and `partContentFingerprint` must both classify through this
 * function. A second classifier lets the fingerprint miss content the tokenizer counts.
 * commentlint: allow(JUDGE)
 */
function classifyNonToolPart(
    part: Record<string, unknown>,
    rules: ProviderPartRules,
): NonToolPartContent {
    const type = partType(part);
    if (rules.skippedTypes.has(type) || (type === "meta" && Object.keys(part).length <= 1)) {
        return { kind: "skip" };
    }
    if (type === "text") {
        if (rules.honorsIgnoredText && part.ignored === true) return { kind: "skip" };
        // Both decoders read only `text`; a `content` field is never text.
        const text = firstStringFieldAllowEmpty(part, ["text"]);
        return text ? { kind: "text", text } : { kind: "skip" };
    }
    if (type === "reasoning" || type === "thinking" || type === "redacted_thinking") {
        // OpenCode `reasoning` parts store text in `text`; Pi `thinking` parts store it in `thinking`.
        // A retained part can carry a stale copy of the other field, so the type decides precedence.
        const fields =
            type === "reasoning"
                ? ["text", "thinking", "content", "reasoning"]
                : ["thinking", "text", "content", "reasoning"];
        const text = firstStringFieldAllowEmpty(part, fields);
        if (text !== null && text.length > 0) return { kind: "reasoning", text };
        const redacted = redactedReasoningData(part);
        if (redacted !== null) return { kind: "reasoning", text: redacted };
        return text !== null ? { kind: "reasoning", text } : { kind: "structured" };
    }
    if (type.length === 0) {
        const reasoningText = firstStringFieldAllowEmpty(part, ["thinking", "reasoning"]);
        if (reasoningText !== null) return { kind: "reasoning", text: reasoningText };
    }
    if (looksImageLike(part)) {
        return { kind: "image", altText: firstStringField(part, ["alt", "text", "description"]) };
    }
    if (type === "file") {
        // Every file part is a media block, whatever inline fields it carries.
        return { kind: "image", altText: firstStringField(part, ["alt", "description"]) };
    }
    return { kind: "structured" };
}

function estimateNonToolPart(
    part: unknown,
    options: TrueRawEstimateOptions,
    breakdown: TrueRawTokenBreakdown,
): void {
    if (!isRecord(part)) {
        if (part !== null && part !== undefined)
            addBreakdown(breakdown, "other", estimateStructured(part));
        return;
    }
    const content = classifyNonToolPart(part, partRulesFor(options.providerShapeVersion));
    switch (content.kind) {
        case "skip":
            return;
        case "text":
            addBreakdown(breakdown, "text", estimateTokens(content.text));
            return;
        case "reasoning":
            addBreakdown(breakdown, "reasoning", estimateTokens(content.text));
            return;
        case "image":
            addBreakdown(breakdown, "image", imageTokensFor(part, options));
            if (content.altText) addBreakdown(breakdown, "text", estimateTokens(content.altText));
            return;
        case "structured":
            addBreakdown(breakdown, "other", estimateStructured(part));
            return;
    }
}

function imageTokensFor(part: unknown, options: TrueRawEstimateOptions): number {
    return options.imageTokenHeuristic?.(part) ?? defaultImageTokenHeuristic(part);
}

/** Each part is serialized independently; `buildToolArcs` queues parts with the same call id separately, so each must be counted. */
export function estimateTrueRawMessageTokens(
    message: RawMessage,
    options: TrueRawEstimateOptions,
): TrueRawTokenBreakdown {
    const breakdown = cloneBreakdown(EMPTY_BREAKDOWN);
    const rules = partRulesFor(options.providerShapeVersion);

    for (const part of message.parts) {
        const signal = toolSignalFromPart(part, rules);
        if (signal) {
            if (signal.hasInput) {
                addBreakdown(breakdown, "toolInput", estimateTokens(signal.inputText));
            }
            if (signal.hasOutput) {
                addBreakdown(breakdown, "toolOutput", estimateTokens(signal.outputText));
                for (const media of signal.outputMedia) {
                    addBreakdown(breakdown, "image", imageTokensFor(media, options));
                }
            }
            continue;
        }
        estimateNonToolPart(part, options, breakdown);
    }
    return breakdown;
}

/**
 * An OpenCode `tool` part with no call id still describes one call, so it needs an identity for its arc.
 * The ordinal and part index make two adjacent idless parts distinct.
 */
function synthesizedToolCallId(ordinal: number, partIndex: number, signal: ToolSignal): string {
    return `synth-tool-${ordinal}-${partIndex}-${signal.toolName || "tool"}-${contentStringsHash([signal.inputText])}`;
}

export function buildToolArcs(
    messages: readonly RawMessage[],
    providerShapeVersion: ProviderShapeVersion,
): ToolArc[] {
    const rules = partRulesFor(providerShapeVersion);
    const openQueues = new Map<string, number[]>();
    const arcs: ToolArc[] = [];
    for (const message of messages) {
        for (const [partIndex, part] of message.parts.entries()) {
            const rawSignal = toolSignalFromPart(part, rules);
            if (!rawSignal) continue;
            // Provider-executed calls cannot leave a local invocation for the fence to protect.
            if (rawSignal.providerExecuted) continue;
            const signal =
                rawSignal.callId.length > 0
                    ? rawSignal
                    : {
                          ...rawSignal,
                          callId: synthesizedToolCallId(message.ordinal, partIndex, rawSignal),
                      };
            if (signal.hasInput && signal.hasOutput) {
                arcs.push({
                    callId: signal.callId,
                    invOrdinal: message.ordinal,
                    resOrdinal: message.ordinal,
                });
                continue;
            }
            if (signal.hasInput) {
                const queue = openQueues.get(signal.callId) ?? [];
                queue.push(message.ordinal);
                openQueues.set(signal.callId, queue);
                continue;
            }
            if (signal.hasOutput) {
                const queue = openQueues.get(signal.callId) ?? [];
                const invOrdinal = queue.shift();
                if (queue.length === 0) openQueues.delete(signal.callId);
                else openQueues.set(signal.callId, queue);
                if (invOrdinal !== undefined) {
                    arcs.push({ callId: signal.callId, invOrdinal, resOrdinal: message.ordinal });
                }
            }
        }
    }
    for (const [callId, queue] of openQueues) {
        for (const invOrdinal of queue) {
            arcs.push({ callId, invOrdinal, resOrdinal: null });
        }
    }
    return arcs.sort(
        (a, b) =>
            a.invOrdinal - b.invOrdinal ||
            (a.resOrdinal ?? Number.MAX_SAFE_INTEGER) - (b.resOrdinal ?? Number.MAX_SAFE_INTEGER),
    );
}

interface CompletedToolArc {
    invOrdinal: number;
    resOrdinal: number;
}

/**
 * Overlapping completed arcs form one atomic interval.
 * When that interval crosses `candidate`, the fence returns its first invocation, keeping the whole interval in the protected tail.
 * The fence returns the ordinal after the interval's last result only when its first invocation is below `publicationFloorOrdinal`, where nothing can be protected.
 */
function fenceBoundaryForCompletedToolArcs(
    candidate: number,
    arcs: readonly ToolArc[],
    publicationFloorOrdinal: number,
): number {
    const completed: CompletedToolArc[] = [];
    for (const arc of arcs) {
        if (arc.resOrdinal !== null) {
            completed.push({ invOrdinal: arc.invOrdinal, resOrdinal: arc.resOrdinal });
        }
    }
    const component = completed.filter((arc) =>
        completedToolArcCrossesBoundary(arc.invOrdinal, arc.resOrdinal, candidate),
    );
    if (component.length === 0) return candidate;

    for (let pass = 0; pass <= completed.length; pass += 1) {
        const minInvocation = Math.min(...component.map((arc) => arc.invOrdinal));
        const maxResult = Math.max(...component.map((arc) => arc.resOrdinal));
        const previousLength = component.length;
        for (const arc of completed) {
            if (
                arc.invOrdinal <= maxResult &&
                arc.resOrdinal >= minInvocation &&
                !component.includes(arc)
            ) {
                component.push(arc);
            }
        }
        if (component.length === previousLength) break;
    }

    const minInvocation = Math.min(...component.map((arc) => arc.invOrdinal));
    const maxResult = Math.max(...component.map((arc) => arc.resOrdinal));
    return minInvocation < publicationFloorOrdinal ? maxResult + 1 : minInvocation;
}

/** `lastCompartmentEndOrdinal + 1` is the publication floor: the first ordinal the fence can still protect. */
export function fenceBoundaryForToolArcs(
    candidate: number,
    arcs: readonly ToolArc[],
    lastCompartmentEndOrdinal: number,
    recentOpenArcCutoff: number,
): number {
    const publicationFloorOrdinal = lastCompartmentEndOrdinal + 1;
    let boundary = fenceBoundaryForCompletedToolArcs(candidate, arcs, publicationFloorOrdinal);
    for (const arc of arcs) {
        // The historian protects only open arcs inside the live protected-tail window.
        // Protecting stale open arcs can block compaction of the eligible region.
        // Compaction emits no dangling `tool_use`.
        if (arc.resOrdinal !== null || arc.invOrdinal < recentOpenArcCutoff) continue;
        if (arc.invOrdinal >= publicationFloorOrdinal || arc.invOrdinal >= boundary) {
            boundary = arc.invOrdinal;
            break;
        }
    }
    // An open-arc boundary can land inside an overlapping completed arc, so the completed fence runs again.
    return fenceBoundaryForCompletedToolArcs(boundary, arcs, publicationFloorOrdinal);
}

function tokenForMessage(
    sessionId: string,
    message: RawMessage,
    options: TrueRawTokenIndexBuildOptions,
): TrueRawTokenBreakdown {
    const key = messageCacheKey(sessionId, message, options);
    const cached = messageEstimateCache.get(key);
    if (cached) return cloneBreakdown(cached.breakdown);
    const breakdown = estimateTrueRawMessageTokens(message, options);
    setCachedEstimate(key, breakdown);
    return cloneBreakdown(breakdown);
}

export function buildTrueRawTokenIndex(
    sessionId: string,
    messages: readonly RawMessage[],
    options: TrueRawTokenIndexBuildOptions,
): TrueRawTokenIndex {
    const ordered = [...messages].sort((a, b) => a.ordinal - b.ordinal);
    // `rawMessageCount` is the absolute session message count.
    // Tail-only callers supply `rawMessageCount` through `absoluteMessageCount`.
    // For whole-session callers, `rawMessageCount` equals `messages.length`.
    const sliceCount = ordered.length;
    const firstOrdinal = ordered.length > 0 ? ordered[0].ordinal : 1;
    const terminalOrdinal = ordered.length > 0 ? ordered[ordered.length - 1].ordinal : 0;
    // Public counts use absolute ordinals; token-sum indexing uses the represented ordinal span.
    // A tail beginning at `prior_last + 1` must index token sums by its represented ordinal span.
    // Using a tail message's absolute ordinal as an array index clamps its lookup to the final element.
    const rawMessageCount = Math.max(
        sliceCount,
        terminalOrdinal,
        options.absoluteMessageCount ?? sliceCount,
    );
    const ordinalSpan = terminalOrdinal >= firstOrdinal ? terminalOrdinal - firstOrdinal + 1 : 0;
    const tokensByOrdinal = new Map<number, number>();
    const idsByOrdinal = new Map<number, string>();
    const prefix = new Array<number>(ordinalSpan + 1).fill(0);
    for (const message of ordered) {
        const stored = options.storedTotalForMessage?.(message);
        const total =
            stored !== undefined && stored !== null
                ? stored
                : tokenForMessage(sessionId, message, options).total;
        tokensByOrdinal.set(message.ordinal, total);
        idsByOrdinal.set(message.ordinal, message.id);
        const relative = message.ordinal - firstOrdinal + 1;
        if (relative >= 1 && relative <= ordinalSpan) {
            prefix[relative] = total;
        }
    }
    for (let k = 1; k <= ordinalSpan; k += 1) {
        prefix[k] += prefix[k - 1];
    }
    const ordinalToIndex = (ordinal: number): number =>
        Math.max(0, Math.min(ordinalSpan, ordinal - firstOrdinal));
    const representedOrdinals = ordered.map((message) => message.ordinal);
    // Malformed rows consume ordinals without yielding messages, so the span can contain holes.
    // A head end must sit right after a real message: never on a hole, never past a hole.
    const lowerBound = (ordinal: number): number => {
        let lo = 0;
        let hi = representedOrdinals.length;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (representedOrdinals[mid] < ordinal) lo = mid + 1;
            else hi = mid;
        }
        return lo;
    };
    const firstRepresentedAtOrAfter = (ordinal: number): number | null => {
        const index = lowerBound(ordinal);
        return index < representedOrdinals.length ? representedOrdinals[index] : null;
    };
    const lastRepresentedBefore = (ordinal: number): number | null => {
        const index = lowerBound(ordinal);
        return index > 0 ? representedOrdinals[index - 1] : null;
    };
    return {
        sessionId,
        providerShapeVersion: options.providerShapeVersion,
        rawMessageCount,
        tokenForOrdinal(ordinal: number): number {
            return tokensByOrdinal.get(ordinal) ?? 0;
        },
        messageIdAtOrdinal(ordinal: number): string | null {
            return idsByOrdinal.get(ordinal) ?? null;
        },
        suffixTokensFromOrdinal(ordinal: number): number {
            if (ordinal <= firstOrdinal) return prefix[ordinalSpan];
            if (ordinal > terminalOrdinal) return 0;
            return prefix[ordinalSpan] - prefix[ordinalToIndex(ordinal)];
        },
        rangeTokens(startInclusive: number, endExclusive: number): number {
            const start = Math.max(firstOrdinal, Math.min(terminalOrdinal + 1, startInclusive));
            const end = Math.max(start, Math.min(terminalOrdinal + 1, endExclusive));
            return prefix[end - firstOrdinal] - prefix[start - firstOrdinal];
        },
        findSuffixStartForTokens(tokens: number): number {
            if (!Number.isFinite(tokens) || tokens <= 0) return terminalOrdinal + 1;
            const target = Math.max(0, Math.floor(tokens));
            const total = prefix[ordinalSpan];
            if (total < target) return firstOrdinal;
            const cut = total - target;
            let lo = 0;
            let hi = ordinalSpan;
            let best = 0;
            while (lo <= hi) {
                const mid = (lo + hi) >> 1;
                if (prefix[mid] <= cut) {
                    best = mid;
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            return firstOrdinal + best;
        },
        findHeadEndForCap(startInclusive: number, endExclusive: number, capTokens: number): number {
            const start = Math.max(firstOrdinal, Math.min(terminalOrdinal + 1, startInclusive));
            const end = Math.max(start, Math.min(terminalOrdinal + 1, endExclusive));
            if (!Number.isFinite(capTokens) || capTokens <= 0) return start;
            const startIndex = start - firstOrdinal;
            const endIndex = end - firstOrdinal;
            const cut = prefix[startIndex] + Math.floor(capTokens);
            let lo = startIndex + 1;
            let hi = endIndex;
            let bestEndIndex = startIndex;
            while (lo <= hi) {
                const mid = (lo + hi) >> 1;
                if (prefix[mid] <= cut) {
                    bestEndIndex = mid;
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            let bestEnd = firstOrdinal + bestEndIndex;
            // The dense prefix includes zero-token holes, so end at the last represented message that fits.
            const lastIncluded = lastRepresentedBefore(bestEnd);
            bestEnd = lastIncluded !== null && lastIncluded >= start ? lastIncluded + 1 : start;
            // The head must contain at least the first represented message at or after `start`,
            // even when that message alone exceeds the cap.
            const firstMessage = firstRepresentedAtOrAfter(start);
            if (firstMessage !== null && firstMessage < end && bestEnd <= firstMessage) {
                bestEnd = firstMessage + 1;
            }
            return Math.min(bestEnd, end);
        },
    };
}

/**
 * `imageTokenHeuristic` receives the whole part and may read any field, so the media
 * fingerprint hashes the whole part rather than a guessed field list.
 */
function mediaFingerprintFields(media: Record<string, unknown>): string[] {
    return ["media", stableStringify(media)];
}

/**
 * The fingerprint hashes the content the tokenizer counts for each part.
 * Text-bearing parts contribute only their counted text, so `updated-at` metadata cannot perturb them.
 * Tool fingerprints include fields consumed by `buildToolArcs` and tool-call summaries, so topology and displayed-name changes invalidate them.
 */
function partContentFingerprint(part: unknown, rules: ProviderPartRules): string {
    if (!isRecord(part)) return nonRecordPartFingerprint(part);
    const tool = toolSignalFromPart(part, rules);
    if (tool) {
        return contentStringsHash([
            "tool",
            tool.callId,
            tool.toolName,
            tool.hasInput ? "in" : "",
            tool.hasOutput ? "out" : "",
            tool.providerExecuted ? "provider" : "",
            tool.isError ? "error" : "",
            tool.inputText,
            tool.outputText,
            tool.metadataDescription,
            ...tool.outputMedia.flatMap(mediaFingerprintFields),
        ]);
    }
    const content = classifyNonToolPart(part, rules);
    switch (content.kind) {
        case "skip":
            return contentStringsHash(["skip"]);
        case "text":
        case "reasoning":
            return contentStringsHash([content.kind, content.text]);
        case "image":
            return contentStringsHash(mediaFingerprintFields(part));
        case "structured":
            return contentStringsHash(["structured", stableStringify(part)]);
    }
}

export function computeRawRangeFingerprint(
    messages: readonly RawMessage[],
    startInclusive: number,
    endExclusive: number,
    providerShapeVersion: ProviderShapeVersion,
): string {
    const rules = partRulesFor(providerShapeVersion);
    const pieces: string[] = [];
    for (const message of messages) {
        if (message.ordinal < startInclusive || message.ordinal >= endExclusive) continue;
        const partFingerprint = message.parts
            .map((part) => partContentFingerprint(part, rules))
            .join(",");
        pieces.push(
            `${message.ordinal}:${message.id}:${message.role}:${message.parts.length}:${partFingerprint}`,
        );
    }
    return pieces.join("|");
}

export function invalidateTrueRawTokenCache(args: {
    sessionId?: string;
    messageId?: string;
    reason:
        | "message.updated"
        | "message.removed"
        | "session.compacted"
        | "session.deleted"
        | "pi.branch.changed"
        | "pi.stable-id-scheme.changed"
        | "provider.unregistered"
        | "schema.migration";
}): void {
    const sessionNeedle = args.sessionId ? `\0${args.sessionId}\0` : null;
    const messageNeedle = args.messageId ? `\0${args.messageId}\0` : null;
    for (const [key, value] of messageEstimateCache) {
        const sessionMatches = sessionNeedle === null || key.includes(sessionNeedle);
        const messageMatches = messageNeedle === null || key.includes(messageNeedle);
        if (sessionMatches && messageMatches) {
            messageEstimateCache.delete(key);
            messageEstimateCacheBytes -= value.keyEstimateBytes;
        }
    }
    void args.reason;
}

export function buildTrueRawTokenIndexFromTokenCountsForTest(
    sessionId: string,
    tokens: readonly number[],
): TrueRawTokenIndex {
    const rawMessageCount = tokens.length;
    const prefix = new Array<number>(rawMessageCount + 1).fill(0);
    for (let index = 0; index < rawMessageCount; index += 1) {
        prefix[index + 1] = prefix[index] + Math.max(0, Math.floor(tokens[index] ?? 0));
    }
    return {
        sessionId,
        providerShapeVersion: "test",
        rawMessageCount,
        tokenForOrdinal(ordinal: number): number {
            return tokens[ordinal - 1] ?? 0;
        },
        messageIdAtOrdinal(ordinal: number): string | null {
            return ordinal >= 1 && ordinal <= rawMessageCount ? `m-${ordinal}` : null;
        },
        suffixTokensFromOrdinal(ordinal: number): number {
            if (ordinal <= 1) return prefix[rawMessageCount];
            if (ordinal > rawMessageCount) return 0;
            return prefix[rawMessageCount] - prefix[ordinal - 1];
        },
        rangeTokens(startInclusive: number, endExclusive: number): number {
            const start = Math.max(1, Math.min(rawMessageCount + 1, startInclusive));
            const end = Math.max(start, Math.min(rawMessageCount + 1, endExclusive));
            return prefix[end - 1] - prefix[start - 1];
        },
        findSuffixStartForTokens(tokensNeeded: number): number {
            if (!Number.isFinite(tokensNeeded) || tokensNeeded <= 0) return rawMessageCount + 1;
            const target = Math.max(0, Math.floor(tokensNeeded));
            const total = prefix[rawMessageCount];
            if (total < target) return 1;
            const cut = total - target;
            let lo = 0;
            let hi = rawMessageCount;
            let best = 0;
            while (lo <= hi) {
                const mid = (lo + hi) >> 1;
                if (prefix[mid] <= cut) {
                    best = mid;
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            return best + 1;
        },
        findHeadEndForCap(startInclusive: number, endExclusive: number, capTokens: number): number {
            const start = Math.max(1, Math.min(rawMessageCount + 1, startInclusive));
            const end = Math.max(start, Math.min(rawMessageCount + 1, endExclusive));
            if (!Number.isFinite(capTokens) || capTokens <= 0) return start;
            const cut = prefix[start - 1] + Math.floor(capTokens);
            let result = start;
            for (let ordinal = start; ordinal < end; ordinal += 1) {
                if (prefix[ordinal] <= cut) result = ordinal + 1;
                else break;
            }
            return result === start && start < end ? start + 1 : result;
        },
    };
}
