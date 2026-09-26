import { createHash, randomUUID } from "node:crypto";

import { DEFAULT_PROTECTED_TAGS } from "../../features/context/defaults";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { piModelRefToCanonical } from "../../shared/harness-provider-map";
import { sessionLog } from "../../shared/logger";
import type { PromptSurfaceConfig } from "../../shared/prompt-surface";
import {
    type PromptSurfaceRuntime,
    promptSurfaceWireFields,
} from "../../shared/prompt-surface-runtime";
import type { WindowGeometryResult } from "../../shared/window-geometry";
import {
    applyRecipe,
    canonicalJsonLength,
    parseRecipe,
    type RecipeSourceBase,
} from "./edit-recipe";
import {
    resolveEidnaraReduceAvailability,
    resolveEidnaraReduceAvailabilityFromMessages,
    resolveTodowriteAvailability,
    resolveTodowriteAvailabilityFromMessages,
    type ToolAvailabilityVerdict,
    todowritePermissionDenied,
} from "./eidnara-reduce-availability";
import type { ContextUsageEntry } from "./event-handler";
import type { ContextUsage } from "./event-payloads";
import {
    resolveCacheTtl,
    resolveContextWindowGeometry,
    resolveExecuteThreshold,
    resolveModelKey,
    resolveTrustedContextLimit,
} from "./event-resolvers";
import { validateInvocation } from "./invocation-budget";
import { isModuleTransportGenerationChangedResult } from "./module-transport";
import {
    annotateOrdinals,
    buildPagedModuleTransformPayloads,
    encodeOpenCodeMessagesToCk,
    type ModuleMethod,
    type ModuleOrdinalMemo,
    ORDINAL_ENTRY_RETAINED_BYTES,
    type OrdinalResolution,
    primeOrdinalMemo,
} from "./module-wire";
import { findLastAssistantModelFromOpenCodeDb, isMidTurn } from "./read-session-db";
import {
    knownSessionDirectory,
    resolveSessionDirectory,
    type SessionDirectoryDeps,
} from "./session-directory";
import type { MessageLike } from "./tag-content-primitives";
import {
    CaptureBudgetExceeded,
    type CapturedHistory,
    type CapturedMessages,
    type CaptureLease,
    capturedMessagesUnchanged,
    captureHistory,
    captureMessages,
    defaultTransformCaptureAdmission,
    type HistoryDigest,
    historyDigestsEqual,
    hostArrayReplacementRejection,
    inspectReferenceableMessages,
    readOwnDataProperty,
    replaceHostArrayContents,
    type TransformCaptureAdmission,
} from "./transform-capture";
import { logTransformTiming } from "./transform-stage-logger";

export interface RustModeTransformDeps extends SessionDirectoryDeps {
    contextUsageMap: BoundedSessionMap<ContextUsageEntry>;
    protectedTags?: number;
    clearReasoningAge: number;
    executeThresholdPercentage?: number | { default: number; [modelKey: string]: number };
    executeThresholdTokens?: { default?: number; [modelKey: string]: number | undefined };
    historyBudgetPercentage?: number;
    promptSurface?: PromptSurfaceConfig;
    promptSurfaceRuntime?: PromptSurfaceRuntime;
    terse_text_compressionTextCompression?: { enabled: boolean; minChars: number };
    autoSearch?: { enabled: boolean; scoreThreshold: number; minPromptChars: number };
    cacheTtl: string | Record<string, string>;
    compactionOff?: boolean;
    isSubagentSession: (sessionId: string) => boolean;
    systemPromptHashFor: (sessionId: string) => string;
    /** A session deleted while its directory resolved declines without dispatch. */
    isSessionDeleted?: (sessionId: string) => boolean;
    onSessionDeletedDuringPreflight?: (sessionId: string) => void;
    /** Hidden `eidnara-` children run Eidnara's own prompts and receive no transform. */
    isInternalChildSession?: (sessionId: string) => boolean;
}

function activeAgentFromMessages(messages: readonly MessageLike[]): string | undefined {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        const info = messages[index]?.info as { role?: unknown; agent?: unknown } | undefined;
        if (info?.role !== "user") continue;
        return typeof info.agent === "string" && info.agent.length > 0 ? info.agent : undefined;
    }
    return undefined;
}

async function resolveCombinedTodowriteVerdict(
    deps: RustModeTransformDeps,
    sessionId: string,
    activeAgent: string | undefined,
    availability: ToolAvailabilityVerdict,
): Promise<boolean> {
    if (!availability.frozen || !availability.callable || deps.compactionOff === true) return false;

    return !(await todowritePermissionDenied(deps.client, sessionId, activeAgent));
}

export interface RustModeModuleClient {
    call(args: {
        sessionId: string;
        projectRoot: string;
        method: ModuleMethod;
        body: unknown;
        signal?: AbortSignal;
        generationSensitive?: boolean;
        /** Capture flush waits up to 20 seconds in the daemon; its transport must outlive that wait. */
        timeoutMs?: number;
    }): Promise<unknown>;
    deleteSession?(sessionId: string, projectRoot: string): Promise<void>;
    closeSession?(sessionId: string): void;
    hasSessionRoute?(sessionId: string): boolean;
}

/** Wire caches hold a session's content snapshots and its last applied output, so the LRU bound is sized to the sessions one OpenCode process keeps active. An evicted session sends its next pass as a full array. */
const WIRE_CACHE_SESSION_CAPACITY = 64;
/** Applied outputs are optional reuse state, budgeted apart from the frame and reconstruction caps. */
const OPTIONAL_OUTPUT_BUDGET_BYTES = 64 * 1024 * 1024;
/** Each retained canonical length occupies one number slot. */
const LENGTH_SLOT_BYTES = 8;
/** Per-message history state kept between passes, budgeted across sessions like applied outputs. */
const RETAINED_HISTORY_BUDGET_BYTES = 64 * 1024 * 1024;
/** Each retained message keeps an input length and a wire bound; the history digest is fixed size. */
const HISTORY_ENTRY_RETAINED_BYTES = 2 * LENGTH_SLOT_BYTES;

/** One successfully applied output, eligible as the `previous` source of the next recipe. */
interface AppliedOutput {
    revision: string;
    values: readonly unknown[];
    lengths: readonly number[];
    capture: CapturedMessages;
    /** Canonical bytes, retained lengths, and snapshots all count against the optional-output budget. */
    charge: number;
}

/**
 * Charges applied outputs across sessions and evicts the least recently retained ones once the
 * total exceeds the budget. A retention larger than the whole budget is refused, and refusal only
 * costs the next pass its `previous` source.
 */
class AppliedOutputBudget {
    private readonly charges = new Map<string, number>();
    private used = 0;

    constructor(
        private readonly capacity: number,
        private readonly evict: (sessionId: string) => void,
    ) {}

    retain(sessionId: string, charge: number): boolean {
        this.release(sessionId);
        if (charge > this.capacity) return false;
        for (const [oldest] of this.charges) {
            if (this.used + charge <= this.capacity) break;
            this.release(oldest);
            this.evict(oldest);
        }
        this.charges.set(sessionId, charge);
        this.used += charge;
        return true;
    }

    release(sessionId: string): void {
        const charge = this.charges.get(sessionId);
        if (charge === undefined) return;
        this.charges.delete(sessionId);
        this.used -= charge;
    }

    get usedBytes(): number {
        return this.used;
    }
}

let baseRevisionCounter = 0;
const baseRevisionNonce = randomUUID().slice(0, 8);

/** Names one pass's submitted input; the daemon echoes it so the recipe binds to that snapshot. */
function nextBaseRevision(): string {
    baseRevisionCounter += 1;
    return `${baseRevisionNonce}-${baseRevisionCounter.toString(36)}`;
}

interface RustWireCache {
    rawCount: number;
    wireCount: number;
    rawLastVisible: boolean;
    /** Digest of every submitted message but the last. Each pass re-verifies the messages it
     * covers, so in-place edits cannot reuse a stale prefix. */
    rawHistory: HistoryDigest;
    /** The former terminal alone; the host may edit it in place, so it is compared separately. */
    rawTerminal?: HistoryDigest;
    /** Inspection wire bounds of the submitted messages, reused for the verified prefix. */
    wireBytes: readonly number[];
    ckFingerprint: string;
    ckPrefixFingerprintBeforeLast: string;
    nativeFingerprint: string;
    nativePrefixFingerprintBeforeLast: string;
    fingerprint: string;
    /** Canonical JSON length of each submitted native message; a delta pass reuses the prefix. */
    inputLengths: readonly number[];
    /** The output the last pass published, offered to the daemon as the next recipe's `previous`. */
    applied?: AppliedOutput;
}

export interface RustSessionState {
    initialized: boolean;
    consecutiveFailures: number;
    passCount: number;
    /** `need_full_sync` forces the next pass to send the full wire array until a pass applies.
     * `need_full_sync` bypasses delta eligibility until a pass applies. */
    forceFullWire: boolean;
    /** `invalidateWireState` increments this value. A pass publishes only when the value
     * matches the one it read alongside the previous cache, so an invalidation that lands during
     * the daemon call rejects that pass's pending output. */
    wireInvalidations: number;
    ordinals: ModuleOrdinalMemo;
    failureCount: number;
    /** Consecutive passes whose newest user message is synthetic. A real user message resets it. */
    syntheticTurnCount: number;
    lastObservedUserMessageId: string | null;
    /** One cascade log per run of synthetic turns; the reset that clears `syntheticTurnCount` re-arms it. */
    syntheticCascadeLogged: boolean;
    routeRoot: string | null;
}

export interface RustModeTransformOptions {
    moduleClient: RustModeModuleClient;
    projectRoot?: string;
    /** Shared admission owner; tests inject one with smaller limits. */
    captureAdmission?: TransformCaptureAdmission;
    /** Retained applied-output budget across sessions; tests inject a smaller one. */
    optionalOutputBudgetBytes?: number;
    /** Retained per-message history budget across sessions; tests inject a smaller one. */
    retainedHistoryBudgetBytes?: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object";
}

function messageInfo(value: unknown): Record<string, unknown> {
    if (!isRecord(value)) return {};
    return isRecord(value.info) ? value.info : value;
}

function messageIdOf(message: MessageLike): string | null {
    const id = messageInfo(message).id;
    return typeof id === "string" && id.length > 0 ? id : null;
}

function advanceWireFingerprint(previous: string, encoded: unknown): string {
    return createHash("sha256")
        .update(previous)
        .update("\\0")
        .update(JSON.stringify(encoded) ?? "null")
        .digest("hex");
}

function buildWireFingerprint(
    encoded: readonly unknown[],
    seed = "rust-wire-v1",
): {
    fingerprint: string;
    prefixFingerprintBeforeLast: string;
} {
    let fingerprint = seed;
    let prefixFingerprintBeforeLast = fingerprint;
    for (let index = 0; index < encoded.length; index += 1) {
        if (index === encoded.length - 1) prefixFingerprintBeforeLast = fingerprint;
        fingerprint = advanceWireFingerprint(fingerprint, encoded[index]);
    }
    return { fingerprint, prefixFingerprintBeforeLast };
}

interface WireDelta {
    rawStart: number;
    wireStart: number;
    after: string;
    ckAfter: string;
    nativeAfter: string;
}

/**
 * Delta transport requires the members before the former terminal to have verified against
 * `previous.rawHistory`. The terminal is compared separately because an invisible former terminal
 * may have been edited in place while a message was appended.
 */
function computeWireDelta(
    previous: RustWireCache,
    captured: CapturedHistory,
): WireDelta | undefined {
    if (captured.members.length < previous.rawCount) return undefined;
    if (!captured.verified || !historyDigestsEqual(captured.verified, previous.rawHistory))
        return undefined;
    const appending = captured.members.length > previous.rawCount;
    const formerTerminalIndex = previous.rawCount - 1;
    const lastChanged =
        formerTerminalIndex >= 0 &&
        !(appending && previous.rawLastVisible) &&
        !formerTerminalUnchanged(previous, captured);
    const replaceExistingTail = lastChanged || (appending && previous.rawLastVisible);
    const rawStart = replaceExistingTail ? Math.max(0, previous.rawCount - 1) : previous.rawCount;
    const replaceExistingWireTail = previous.rawLastVisible && (lastChanged || appending);
    const wireStart = replaceExistingWireTail
        ? Math.max(0, previous.wireCount - 1)
        : previous.wireCount;
    const ckAfter =
        wireStart === previous.wireCount - 1
            ? previous.ckPrefixFingerprintBeforeLast
            : wireStart === previous.wireCount
              ? previous.ckFingerprint
              : undefined;
    const nativeAfter =
        rawStart === previous.rawCount - 1
            ? previous.nativePrefixFingerprintBeforeLast
            : rawStart === previous.rawCount
              ? previous.nativeFingerprint
              : undefined;
    if (ckAfter === undefined || nativeAfter === undefined) return undefined;
    return { rawStart, wireStart, ckAfter, nativeAfter, after: previous.fingerprint };
}

/**
 * A failed pass may serve the retained output only when every message the previous pass submitted,
 * its terminal included, is unchanged and in place. New messages may follow; an edit, removal,
 * revert, or reorder of an acknowledged message leaves the retained output stale.
 */
function isAppendOnlyExtension(previous: RustWireCache, captured: CapturedHistory): boolean {
    return (
        captured.members.length >= previous.rawCount &&
        captured.verified !== undefined &&
        historyDigestsEqual(captured.verified, previous.rawHistory) &&
        (previous.rawCount === 0 || formerTerminalUnchanged(previous, captured))
    );
}

/** A verified capture tapes the former terminal first, so its boundary is that member. */
function formerTerminalUnchanged(previous: RustWireCache, captured: CapturedHistory): boolean {
    return (
        previous.rawTerminal !== undefined &&
        captured.boundary !== undefined &&
        historyDigestsEqual(captured.boundary, previous.rawTerminal)
    );
}

/**
 * The fail-open array and the raw input share the appended suffix, so the fail-open array is
 * larger than raw exactly when the retained output holds more bytes than its source prefix.
 */
function appliedOutputGrew(previous: RustWireCache, applied: AppliedOutput): boolean {
    let appliedBytes = 0;
    for (const length of applied.lengths) appliedBytes += length;
    let prefixBytes = 0;
    for (const length of previous.inputLengths) prefixBytes += length;
    return appliedBytes > prefixBytes;
}

/** The pending cache for a pass; `applied` is attached on publication. */
function buildWireCache(args: {
    messages: readonly MessageLike[];
    encoded: readonly { mid?: unknown }[];
    captured: CapturedHistory;
    wireBytes: readonly number[];
    inputLengths: readonly number[];
    delta?: WireDelta;
}): RustWireCache {
    const { messages, encoded, captured, wireBytes, inputLengths, delta } = args;
    const rawLast = messages.at(-1);
    const ck = buildWireFingerprint(encoded, delta?.ckAfter);
    const native = buildWireFingerprint(
        delta ? messages.slice(delta.rawStart) : messages,
        delta?.nativeAfter,
    );
    return {
        rawCount: messages.length,
        wireCount: (delta?.wireStart ?? 0) + encoded.length,
        rawLastVisible:
            rawLast !== undefined && encoded.some((entry) => entry.mid === messageIdOf(rawLast)),
        ckFingerprint: ck.fingerprint,
        ckPrefixFingerprintBeforeLast: ck.prefixFingerprintBeforeLast,
        nativeFingerprint: native.fingerprint,
        nativePrefixFingerprintBeforeLast: native.prefixFingerprintBeforeLast,
        rawHistory: captured.history,
        ...(captured.terminal ? { rawTerminal: captured.terminal } : {}),
        wireBytes,
        fingerprint: `${ck.fingerprint}|${native.fingerprint}`,
        inputLengths,
    };
}

/**
 * Lengths for the submitted native array: the acknowledged prefix keeps the lengths measured when
 * it was first sent, and only the suffix this pass sends is measured.
 */
function measureInputLengths(
    messages: readonly unknown[],
    previous: RustWireCache | undefined,
    rawStart: number,
): number[] {
    const lengths = previous ? previous.inputLengths.slice(0, rawStart) : [];
    for (let index = lengths.length; index < messages.length; index += 1)
        lengths.push(canonicalJsonLength(messages[index]));
    return lengths;
}

function newestUserMessage(messages: MessageLike[]): MessageLike | undefined {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        if (messageInfo(messages[index]).role === "user") return messages[index];
    }
    return undefined;
}

interface RustPassTimings {
    prefixGuard: number;
    ordinalResolve: number;
    clone: number;
    wireBuild: number;
    wireMessages: number;
    transport: number;
    transportPages: number;
    transportBytes: number;
    apply: number;
}

function emptyRustPassTimings(): RustPassTimings {
    return {
        prefixGuard: 0,
        ordinalResolve: 0,
        clone: 0,
        wireBuild: 0,
        wireMessages: 0,
        transport: 0,
        transportPages: 0,
        transportBytes: 0,
        apply: 0,
    };
}

function formatRustPassLog(args: {
    decision: string;
    reason: string;
    servedFrom: string;
    inputCount: number;
    outputCount: number;
    applied: boolean;
    elapsedMs: number;
    moduleElapsedMs: number;
    rowVersion: number;
    emergencyWaitMs?: number;
    timings?: RustPassTimings;
}): string {
    const timings = args.timings ?? emptyRustPassTimings();
    const measured =
        timings.prefixGuard +
        timings.ordinalResolve +
        timings.clone +
        timings.wireBuild +
        timings.transport +
        timings.apply;
    const unattributed = Math.max(0, args.elapsedMs - measured);
    const rowVersion = Number.isSafeInteger(args.rowVersion) ? args.rowVersion : 0;
    return `rust pass: decision=${args.decision} reason=${args.reason} served_from=${args.servedFrom} in=${args.inputCount} out=${args.outputCount} applied=${args.applied} row_version=${rowVersion} emergency_wait=${(args.emergencyWaitMs ?? 0).toFixed(1)} elapsed=${args.elapsedMs.toFixed(1)} ms module=${args.moduleElapsedMs.toFixed(1)} ms stages=prefix_guard:${timings.prefixGuard.toFixed(1)} ordinal_resolve:${timings.ordinalResolve.toFixed(1)} clone:${timings.clone.toFixed(1)} wire_build:${timings.wireBuild.toFixed(1)} wire_messages:${timings.wireMessages} transport:${timings.transport.toFixed(1)} transport_pages:${timings.transportPages} transport_bytes:${timings.transportBytes} apply:${timings.apply.toFixed(1)} other:${unattributed.toFixed(1)}`;
}

function isSyntheticUserMessage(message: MessageLike | undefined): boolean {
    if (!message || messageInfo(message).role !== "user" || !Array.isArray(message.parts)) {
        return false;
    }
    return (
        message.parts.length > 0 &&
        message.parts.every(
            (part) => isRecord(part) && (part.synthetic === true || part.ignored === true),
        )
    );
}

function observeSyntheticTurn(state: RustSessionState, messages: MessageLike[]): boolean {
    const newest = newestUserMessage(messages);
    const info = messageInfo(newest);
    const messageId = typeof info.id === "string" ? info.id : null;
    const synthetic = isSyntheticUserMessage(newest);
    const isNewMessage = messageId === null || messageId !== state.lastObservedUserMessageId;

    if (!synthetic) {
        state.syntheticTurnCount = 0;
        state.syntheticCascadeLogged = false;
    } else if (isNewMessage) {
        state.syntheticTurnCount += 1;
    }
    state.lastObservedUserMessageId = messageId;
    return synthetic;
}

function assertNativeBoundary(output: unknown[], sessionId: string, boundaryId: string): void {
    const first = output.find((message) => messageInfo(message).role !== "system");
    const info = messageInfo(first);
    const parts = isRecord(first) && Array.isArray(first.parts) ? first.parts : [];
    const synthetic =
        parts.length > 0 && parts.every((part) => isRecord(part) && part.synthetic === true);
    if (info.role === "user" && info.sessionID === sessionId && synthetic) return;
    // The failure arm logs the failed clause and actual head so violations are distinguishable.
    const headSummary = output.slice(0, 3).map((message) => {
        const mi = messageInfo(message);
        const mParts = isRecord(message) && Array.isArray(message.parts) ? message.parts : [];
        const partDesc = mParts
            .slice(0, 5)
            .map((part) =>
                isRecord(part) ? `${String(part.type)}${part.synthetic === true ? "" : "!"}` : "?",
            )
            .join(",");
        return `role=${String(mi.role)} sid=${mi.sessionID === sessionId ? "ok" : String(mi.sessionID ?? "absent")} id=${String(mi.id ?? "-").slice(0, 24)} parts=[${partDesc}]`;
    });
    throw new Error(
        `rust transform wire invariant failed: boundary=${boundaryId} expected a synthetic m0 user message scoped to session ${sessionId}; head: ${headSummary.join(" | ")}`,
    );
}

function responseValue(response: unknown): Record<string, unknown> {
    if (isRecord(response) && isRecord(response.result)) return response.result;
    if (isRecord(response)) return response;
    throw new Error("module transform returned a non-object response");
}

function isTransformPageAttemptMismatch(error: unknown): boolean {
    let current = error;
    const seen = new Set<unknown>();
    while (isRecord(current) && !seen.has(current)) {
        seen.add(current);
        const code = typeof current.code === "string" ? current.code : "";
        const message = typeof current.message === "string" ? current.message : "";
        if (
            code === "attempt_mismatch" ||
            code === "authority_transform_page_attempt_mismatch" ||
            /\b(?:authority_transform_page_)?attempt_mismatch\b/.test(message)
        ) {
            return true;
        }
        current = current.cause;
    }
    return false;
}

function noteDeliveryPassIds(response: Record<string, unknown>): string[] {
    if (!Array.isArray(response.note_deliveries)) return [];
    return [
        ...new Set(
            response.note_deliveries.flatMap((delivery) => {
                if (!isRecord(delivery)) return [];
                const passId = delivery.transform_pass_id;
                return typeof passId === "string" && passId.length > 0 ? [passId] : [];
            }),
        ),
    ];
}

function modelFromMessages(
    messages: MessageLike[],
): { providerID: string; modelID: string } | undefined {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        const info = messages[index]?.info as Record<string, unknown> | undefined;
        const model = isRecord(info?.model) ? info.model : undefined;
        if (typeof model?.providerID === "string" && typeof model.modelID === "string") {
            return { providerID: model.providerID, modelID: model.modelID };
        }
        if (
            typeof info?.providerID === "string" &&
            typeof info.modelID === "string" &&
            info.role === "assistant"
        ) {
            return { providerID: info.providerID, modelID: info.modelID };
        }
    }
    return undefined;
}

function ensureState(states: Map<string, RustSessionState>, sessionId: string): RustSessionState {
    let state = states.get(sessionId);
    if (!state) {
        state = {
            initialized: false,
            consecutiveFailures: 0,
            passCount: 0,
            forceFullWire: false,
            wireInvalidations: 0,
            ordinals: { generation: 0, memoGeneration: 0, entries: new Map() },
            failureCount: 0,
            syntheticTurnCount: 0,
            lastObservedUserMessageId: null,
            syntheticCascadeLogged: false,
            routeRoot: null,
        };
        states.set(sessionId, state);
    }
    return state;
}

function loadContextUsage(
    deps: RustModeTransformDeps,
    sessionId: string,
): ContextUsage | undefined {
    return deps.contextUsageMap.get(sessionId)?.usage;
}

function resolveHistoryBudgetTokens(
    historyBudgetPercentage: number | undefined,
    contextUsage: ContextUsage,
    executeThresholdPercentage:
        | number
        | { default: number; [modelKey: string]: number }
        | undefined,
    modelKey: string | undefined,
    executeThresholdTokens?: { default?: number; [modelKey: string]: number | undefined },
    resolvedContextLimit?: number,
): number | undefined {
    if (!historyBudgetPercentage) {
        return undefined;
    }

    let contextLimit = resolvedContextLimit && resolvedContextLimit > 0 ? resolvedContextLimit : 0;
    if (contextLimit <= 0) {
        if (contextUsage.percentage <= 0) {
            return undefined;
        }
        contextLimit = contextUsage.inputTokens / (contextUsage.percentage / 100);
    }
    if (!Number.isFinite(contextLimit) || contextLimit <= 0) {
        return undefined;
    }

    return Math.floor(
        contextLimit *
            (resolveExecuteThreshold(executeThresholdPercentage ?? 65, modelKey, 65, {
                tokensConfig: executeThresholdTokens,
                contextLimit,
            }) /
                100) *
            historyBudgetPercentage,
    );
}

function passUsage(usage: ContextUsage, limit: number): Record<string, number> {
    return {
        current_total_input_tokens: usage.inputTokens,
        context_limit_tokens: limit,
    };
}

/** The previous response's reported cache counts, from the same snapshot as its pressure usage; absent until a response reports them. */
function prevResponseCacheUsage(
    usage: ContextUsage | undefined,
): { cache_read_tokens: number; cache_write_tokens: number } | undefined {
    const cache = usage?.cache;
    if (!cache || !isCount(cache.readTokens) || !isCount(cache.writeTokens)) return undefined;
    return { cache_read_tokens: cache.readTokens, cache_write_tokens: cache.writeTokens };
}

function isCount(value: number): boolean {
    return Number.isSafeInteger(value) && value >= 0;
}

interface TransformGeometryWire {
    usable_soft: number;
    usable_hard: number;
    derivation: string;
}

function transformGeometryForWire(
    geometry: WindowGeometryResult | undefined,
): TransformGeometryWire | undefined {
    if (!geometry) return undefined;
    const { window, reserve } = geometry.derivation;
    let derivation: string;
    if (geometry.geometry === "separate" && geometry.usableSoft < geometry.usableHard) {
        derivation = `s1-pre-carve/input=${geometry.usableSoft}`;
    } else if (geometry.geometry === "separate") {
        derivation = `s1-separate/context=${window}`;
    } else {
        derivation =
            `s1-shared/context-output/context=${window}/output=${reserve}` +
            `/mode=${geometry.geometry}/usable-hard=${geometry.usableHard}`;
    }
    return {
        usable_soft: geometry.usableSoft,
        usable_hard: geometry.usableHard,
        derivation,
    };
}

function isNeedFullSync(response: Record<string, unknown>): boolean {
    return response.status === "need_full_sync" || response.action === "NEED_FULL_SYNC";
}

/**
 * Applies a `status: ok` response's recipe against the submitted input and, when the daemon named
 * it, the retained previous output. The result is a fresh array of shared references, sized and
 * validated before any allocation; nothing is published on failure.
 */
export function applyTransformRecipe(
    response: Record<string, unknown>,
    input: RecipeSourceBase,
    previous: RecipeSourceBase | undefined,
    reserve: (slots: number, insertedSlots: number) => boolean,
): { values: unknown[]; lengths: number[]; bytes: number; outputRevision: string } {
    const parsed = parseRecipe(response);
    if (!parsed.ok) {
        throw new Error(
            `rust transform recipe rejected: ${parsed.rejection.code}: ${parsed.rejection.detail}`,
        );
    }
    let slots = 0;
    let insertedSlots = 0;
    for (const operation of parsed.recipe.operations) {
        slots += operation.op === "keep" ? operation.count : operation.values.length;
        if (operation.op === "insert") insertedSlots += operation.values.length;
    }
    if (!Number.isSafeInteger(slots) || !reserve(slots, insertedSlots))
        throw new CaptureBudgetExceeded("recipe output array");
    const applied = applyRecipe(parsed.recipe, input, previous);
    if (!applied.ok) {
        throw new Error(
            `rust transform recipe rejected: ${applied.rejection.code}: ${applied.rejection.detail}`,
        );
    }
    return { ...applied, outputRevision: parsed.recipe.outputRevision };
}

function buildTransformBody(args: {
    sessionId: string;
    baseRevision: string;
    previousOutputRevision?: string;
    input: unknown[];
    nativeMessages: unknown[];
    passInputs: Record<string, unknown>;
    usage?: Record<string, number | boolean>;
    prevResponseCacheUsage?: { cache_read_tokens: number; cache_write_tokens: number };
    geometry?: TransformGeometryWire;
    modelKey: string | null;
    providerId: string | null;
    systemPromptHash: string;
    midTurn: boolean;
    prevResponseCompletedAtMs?: number;
    requestObservedAtMs?: number;
    fullArrayFingerprint?: string;
    tailDelta?: {
        after: string;
        replaceFrom: number;
        nativeReplaceFrom: number;
    };
}): Record<string, unknown> {
    return {
        method: "transform",
        kind: "transform",
        v: 2,
        serializer_profile: "opencode-aisdk",
        serve_native: true,
        session_id: args.sessionId,
        base_revision: args.baseRevision,
        ...(args.previousOutputRevision
            ? { previous_output_revision: args.previousOutputRevision }
            : {}),
        // Model, provider, and system-prompt changes evict provider caches; send the native module the identity inputs used by the TypeScript materializer rather than leaving the native identity blank.
        render_config: [
            args.providerId ? `provider:${args.providerId}` : "",
            args.modelKey ? `model:${args.modelKey}` : "",
            args.systemPromptHash ? `system:${args.systemPromptHash}` : "",
        ]
            .filter(Boolean)
            .join("|"),
        system_prompt_hash: args.systemPromptHash,
        upgrade_state: "",
        is_subagent: args.passInputs.is_subagent === true,
        protected_tags: args.passInputs.protected_tags ?? DEFAULT_PROTECTED_TAGS,
        messages: args.input,
        native_messages: args.nativeMessages,
        ...(args.fullArrayFingerprint ? { full_array_fingerprint: args.fullArrayFingerprint } : {}),
        ...(args.tailDelta
            ? {
                  tail_delta: {
                      after: args.tailDelta.after,
                      replace_from: args.tailDelta.replaceFrom,
                      native_replace_from: args.tailDelta.nativeReplaceFrom,
                  },
              }
            : {}),
        ...(args.usage ? { usage: args.usage } : {}),
        ...(args.prevResponseCacheUsage
            ? { prev_response_cache_usage: args.prevResponseCacheUsage }
            : {}),
        ...(args.geometry ? { geometry: args.geometry } : {}),
        mid_turn: args.midTurn,
        prev_response_completed_at_ms: args.prevResponseCompletedAtMs,
        request_observed_at_ms: args.requestObservedAtMs,
        channel2_nudge_state: "",
        emergency_recovery_armed: false,
        emergency_recovery_no_head_escape: false,
        model_key: args.modelKey,
        provider_id: args.providerId,
        tool_present: args.passInputs.tool_present === true,
        ...(typeof args.passInputs.todo_tool_present === "boolean"
            ? { todo_tool_present: args.passInputs.todo_tool_present }
            : {}),
        prompt_surface_preset: args.passInputs.prompt_surface_preset ?? "full",
        prompt_surface_model_key: args.passInputs.prompt_surface_model_key,
        prompt_surface_config_identity: args.passInputs.prompt_surface_config_identity,
        prompt_surface_tool_descriptions: args.passInputs.prompt_surface_tool_descriptions ?? {},
        prompt_surface_guidance_override: args.passInputs.prompt_surface_guidance_override,
        effective_execute_threshold: args.passInputs.effective_execute_threshold,
        auto_search_enabled: args.passInputs.auto_search_enabled === true,
        auto_search_score_threshold: args.passInputs.auto_search_score_threshold,
        auto_search_min_prompt_chars: args.passInputs.auto_search_min_prompt_chars,
        history_budget_tokens: args.passInputs.history_budget_tokens,
        clear_reasoning_age: args.passInputs.clear_reasoning_age,
        terse_text_compression_enabled: args.passInputs.terse_text_compression_enabled === true,
        terse_text_compression_min_chars: args.passInputs.terse_text_compression_min_chars ?? 500,
        cache_ttl: args.passInputs.cache_ttl,
    };
}

const CANDIDATE_SLOT_BYTES = 8;
/** Charged on top of the heuristic estimate because the estimator undercounts relative to the provider's tokenizer. */
const INVOCATION_HEADROOM_PERMILLE = 250;
/** WIRE_PROJECTION_FACTOR accounts for the CK text, the native text, the paging parse copy, and the page texts. */
const WIRE_PROJECTION_FACTOR = 4;

type PassDeclineReason =
    | "cleared"
    | "superseded"
    | "capture_bytes"
    | "invocation_budget"
    | "unsupported_source"
    | "host_container"
    | "source_changed"
    | "invalidated"
    | "deleted"
    | "internal_child"
    | "daemon_session_busy";

/**
 * A local refusal prevents publication without counting a daemon failure. Byte pressure and a
 * polluted built-in prototype recur on every call for the affected session, so they log at warn.
 */
class PassDeclined extends Error {
    readonly logLevel: "debug" | "warn";
    constructor(
        sessionId: string,
        readonly reason: PassDeclineReason,
        detail?: string,
        logLevel: "debug" | "warn" = reason === "capture_bytes" ? "warn" : "debug",
    ) {
        super(`rust session ${sessionId} pass declined: ${reason}${detail ? ` (${detail})` : ""}`);
        this.logLevel = logLevel;
    }
}

interface DeliveryPlan {
    sessionId: string;
    projectRoot: string;
    attempted: Set<string>;
    applied: Set<string>;
}

async function deliverTransformNotes(
    moduleClient: RustModeModuleClient,
    plan: DeliveryPlan,
): Promise<void> {
    for (const disposition of ["nack", "ack"] as const) {
        const errors: unknown[] = [];
        for (const id of plan.attempted) {
            if (plan.applied.has(id) !== (disposition === "ack")) continue;
            const method = `transform.${disposition}` as const;
            try {
                await moduleClient.call({
                    sessionId: plan.sessionId,
                    projectRoot: plan.projectRoot,
                    method,
                    body: { method, v: 1, session_id: plan.sessionId, transform_pass_id: id },
                });
            } catch (error) {
                errors.push(error);
            }
        }
        if (errors.length > 0) {
            sessionLog.warn(
                plan.sessionId,
                `rust note delivery ${disposition} failed (${disposition === "ack" ? "will retry" : "ignored"}):`,
                new AggregateError(errors, `${errors.length} delivery disposition(s) failed`),
            );
        }
    }
}

export function createRustModeTransform(
    deps: RustModeTransformDeps,
    options: RustModeTransformOptions,
): {
    run: (sessionId: string, output: { messages: unknown[] }) => Promise<void>;
    clearSession: (sessionId: string) => void;
    invalidateWireState: (sessionId: string) => void;
    getState: (sessionId: string) => Readonly<RustSessionState>;
} {
    const states = new Map<string, RustSessionState>();
    const wireCaches = new BoundedSessionMap<RustWireCache>(WIRE_CACHE_SESSION_CAPACITY);
    const appliedOutputs = new AppliedOutputBudget(
        options.optionalOutputBudgetBytes ?? OPTIONAL_OUTPUT_BUDGET_BYTES,
        (sessionId) => {
            const cache = wireCaches.peek(sessionId);
            if (cache) cache.applied = undefined;
        },
    );
    /** A session whose history is evicted loses its wire cache and captures in full next pass. */
    const retainedHistories = new AppliedOutputBudget(
        options.retainedHistoryBudgetBytes ?? RETAINED_HISTORY_BUDGET_BYTES,
        (sessionId) => {
            wireCaches.delete(sessionId);
            appliedOutputs.release(sessionId);
        },
    );
    const releaseWireCache = (sessionId: string): void => {
        wireCaches.delete(sessionId);
        appliedOutputs.release(sessionId);
        retainedHistories.release(sessionId);
    };
    /** A digest keeps each symbol, and so its description, alive with the wire cache. */
    const retainedSymbolBytes = (digest: HistoryDigest | undefined): number => {
        let bytes = 0;
        for (const symbol of digest?.symbols ?? [])
            bytes += CANDIDATE_SLOT_BYTES + (symbol.description?.length ?? 0) * 2;
        return bytes;
    };
    /** Count eviction uses `releaseWireCache`: a history eviction can free the slot `set` would evict, and a refused retention skips `set`. */
    const storeWireCache = (sessionId: string, cache: RustWireCache): void => {
        if (!wireCaches.has(sessionId) && wireCaches.size >= WIRE_CACHE_SESSION_CAPACITY) {
            const oldest = wireCaches.entries().next().value;
            if (oldest) releaseWireCache(oldest[0]);
        }
        const charge =
            cache.rawCount * HISTORY_ENTRY_RETAINED_BYTES +
            retainedSymbolBytes(cache.rawHistory) +
            retainedSymbolBytes(cache.rawTerminal);
        if (!retainedHistories.retain(sessionId, charge)) {
            releaseWireCache(sessionId);
            return;
        }
        wireCaches.set(sessionId, cache);
    };
    const captureAdmission = options.captureAdmission ?? defaultTransformCaptureAdmission;

    const logStage = (
        sessionId: string,
        stage: keyof RustPassTimings,
        startedAt: number,
        timings: RustPassTimings,
        extra?: string,
    ): void => {
        const elapsed = Math.max(0, performance.now() - startedAt);
        timings[stage] += elapsed;
        logTransformTiming(
            sessionId,
            `rust.${stage.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`)}`,
            startedAt,
            extra,
        );
    };

    const markFailure = (
        sessionId: string,
        state: RustSessionState,
        error: unknown,
        servedLastApplied: boolean,
    ): void => {
        state.consecutiveFailures += 1;
        state.failureCount += 1;
        sessionLog.warn(
            sessionId,
            servedLastApplied
                ? "rust transform failed; serving the last applied output with the messages appended since:"
                : "rust transform failed; serving the input unchanged:",
            error,
        );
    };

    const invalidateWireState = (sessionId: string): void => {
        releaseWireCache(sessionId);
        captureAdmission.requestCancel(
            sessionId,
            `rust session ${sessionId} wire state invalidated`,
        );
        const state = states.get(sessionId);
        if (!state) return;
        state.ordinals = {
            ...state.ordinals,
            entries: new Map(),
            anchor: null,
            storedCount: null,
            canonicalCount: 0,
        };
        state.forceFullWire = true;
        state.wireInvalidations += 1;
    };

    const execute = async (
        sessionId: string,
        output: { messages: unknown[] },
        lease: CaptureLease,
    ): Promise<DeliveryPlan> => {
        const passStartedAt = performance.now();
        const deliveries: DeliveryPlan = {
            sessionId,
            projectRoot: "",
            attempted: new Set(),
            applied: new Set(),
        };
        const target = readOwnDataProperty(output, "messages") as unknown[];
        const hostRejection = hostArrayReplacementRejection(target);
        if (hostRejection !== null) {
            sessionLog[hostRejection === "prototype_accessor" ? "warn" : "debug"](
                sessionId,
                `rust transform declined before dispatch: host_container ${hostRejection}`,
            );
            return deliveries;
        }
        const state = ensureState(states, sessionId);
        const timings = emptyRustPassTimings();
        let inputCount = 0;
        let decision = "error";
        let materializeReason = "none";
        let servedFrom = "none";
        let moduleElapsedMs = 0;
        let emergencyWaitMs = 0;
        let rowVersion = 0;
        let appliedAt: number | undefined;
        const finishPass = (applied: boolean): void => {
            const elapsedAt = applied && appliedAt !== undefined ? appliedAt : performance.now();
            const elapsedMs = Math.max(0, elapsedAt - passStartedAt);
            sessionLog.debug(
                sessionId,
                formatRustPassLog({
                    decision,
                    reason: materializeReason,
                    servedFrom,
                    inputCount,
                    outputCount: target.length,
                    applied,
                    elapsedMs,
                    moduleElapsedMs,
                    rowVersion,
                    emergencyWaitMs,
                    timings,
                }),
            );
        };
        const captureResponseTelemetry = (response: Record<string, unknown>): void => {
            decision =
                typeof response.decision === "string"
                    ? response.decision
                    : typeof response.action === "string"
                      ? response.action
                      : typeof response.status === "string"
                        ? response.status
                        : "unknown";
            servedFrom =
                typeof response.served_from === "string" ? response.served_from : "unknown";
            materializeReason =
                typeof response.materialize_reason === "string" &&
                response.materialize_reason.length > 0
                    ? response.materialize_reason
                    : "none";
            const timings = isRecord(response.timings) ? response.timings : undefined;
            const applyOnceTotal = timings?.total;
            emergencyWaitMs =
                typeof timings?.emergency_wait === "number" &&
                Number.isFinite(timings.emergency_wait)
                    ? timings.emergency_wait
                    : 0;
            const handlerTotal = timings?.handler_total;
            moduleElapsedMs =
                typeof handlerTotal === "number" && Number.isFinite(handlerTotal)
                    ? handlerTotal
                    : typeof applyOnceTotal === "number" && Number.isFinite(applyOnceTotal)
                      ? applyOnceTotal
                      : 0;
            rowVersion =
                typeof response.row_version === "number" &&
                Number.isSafeInteger(response.row_version)
                    ? response.row_version
                    : 0;
            if (
                timings &&
                (typeof timings.handler_total === "number" ||
                    typeof timings.native_cache_reused_messages === "number" ||
                    typeof timings.native_cache_encoded_messages === "number")
            ) {
                const stage = (name: string): string => {
                    const value = timings[name];
                    return typeof value === "number" && Number.isFinite(value)
                        ? value.toFixed(1)
                        : "n/a";
                };
                sessionLog.debug(
                    sessionId,
                    `rust module stages: handler=${stage("handler_total")} apply_once=${stage("total")} ` +
                        `request_to_handler=${stage("request_observed_to_handler")} delta_expand=${stage("delta_expand")} ` +
                        `projection_cache_lookup=${stage("projection_cache_lookup")} projection=${stage("projection")} ` +
                        `selection=${stage("selection")} build_output=${stage("build_output")} ` +
                        `store_commit=${stage("store_commit")} trigger=${stage("trigger_ms")} ` +
                        `trigger_boundary=${stage("trigger_boundary_build")} trigger_eval=${stage("trigger_eval")} ` +
                        `projection_cache_store=${stage("projection_cache_store")} native_attach=${stage("native_attach")} ` +
                        `retained_size=${stage("retained_size")} snapshot_store=${stage("snapshot_store")} ` +
                        `post_attach=${stage("post_attach")} response_encode=${stage("response_encode")} ` +
                        `response_meta_encode=${stage("response_meta_encode")} response_splice=${stage("response_splice")} ` +
                        `native_cache_reused=${stage("native_cache_reused_messages")} ` +
                        `native_cache_encoded=${stage("native_cache_encoded_messages")}`,
                );
            }
            // Log numeric timing fields to identify the slow stage.
            if (timings && moduleElapsedMs >= 1000) {
                const detail = Object.entries(timings)
                    .filter(
                        ([key, value]) =>
                            key !== "total" && key !== "handler_total" && typeof value === "number",
                    )
                    .map(([key, value]) => `${key}:${(value as number).toFixed(1)}`)
                    .join(" ");
                if (detail)
                    sessionLog.debug(sessionId, `rust module stages (slow pass): ${detail}`);
            }
        };
        state.passCount += 1;
        const wireInvalidationsAtRead = state.wireInvalidations;
        // Clearing a session replaces its state object; supersession, clearing, and wire invalidation abort the capture lease.
        const assertCurrentPass = (): void => {
            if (states.get(sessionId) !== state) throw new PassDeclined(sessionId, "cleared");
            if (state.wireInvalidations !== wireInvalidationsAtRead)
                throw new PassDeclined(sessionId, "invalidated");
            if (lease.signal.aborted) throw new PassDeclined(sessionId, "superseded");
        };
        const charge = (bytes: number, detail: string): void => {
            if (!lease.reserve(bytes)) throw new CaptureBudgetExceeded(detail);
        };
        let failOpenSource: { captured: CapturedHistory; previous: RustWireCache } | undefined;
        /**
         * Without native compaction a raw fail-open can overflow the provider window, so a failed
         * pass republishes the last applied output followed by the raw messages appended after the
         * prefix that output was computed from. Messages are appended whole, so a tool part keeps
         * its call and result together. Any doubt about the prefix serves the input unchanged.
         */
        const serveLastApplied = (): boolean => {
            const source = failOpenSource;
            const applied = source?.previous.applied;
            if (!source || !applied) return false;
            try {
                if (
                    states.get(sessionId) !== state ||
                    state.wireInvalidations !== wireInvalidationsAtRead ||
                    lease.signal.aborted ||
                    wireCaches.peek(sessionId) !== source.previous ||
                    !isAppendOnlyExtension(source.previous, source.captured) ||
                    appliedOutputGrew(source.previous, applied) ||
                    readOwnDataProperty(output, "messages") !== target ||
                    !capturedMessagesUnchanged(target, source.captured) ||
                    hostArrayReplacementRejection(target) !== null
                )
                    return false;
                if (!capturedMessagesUnchanged(applied.values, applied.capture)) {
                    source.previous.applied = undefined;
                    appliedOutputs.release(sessionId);
                    return false;
                }
                const served = [
                    ...applied.values,
                    ...source.captured.members.slice(source.previous.rawCount),
                ];
                if (!lease.reserve(served.length * CANDIDATE_SLOT_BYTES)) return false;
                replaceHostArrayContents(target, served);
                return true;
            } catch (error) {
                sessionLog.warn(sessionId, "rust transform fail-open reuse declined:", error);
                return false;
            }
        };
        try {
            // Source domain is validated synchronously before any message read.
            const prefixGuardStartedAt = performance.now();
            const previousWireCache = wireCaches.get(sessionId);
            // A full fallback inspection walks a superset of the partial one, so it pays only the difference.
            let inspectedBytes = 0;
            const inspect = (skip: number): number[] => {
                const inspection = inspectReferenceableMessages(
                    target,
                    lease.remainingBytes + inspectedBytes,
                    skip,
                );
                if (!inspection.ok) {
                    throw new PassDeclined(
                        sessionId,
                        "unsupported_source",
                        `${inspection.rejection.reason} at ${inspection.rejection.path}`,
                        inspection.rejection.reason === "prototype_accessor" ? "warn" : "debug",
                    );
                }
                charge(
                    inspection.estimatedBytes - inspectedBytes,
                    `capture charge=${inspection.estimatedBytes}`,
                );
                inspectedBytes = inspection.estimatedBytes;
                return inspection.messageWireBytes;
            };
            /**
             * The acknowledged history before the former terminal is matched against its digest
             * rather than re-taped, so the charge covers the root and the messages after it. The
             * former terminal is taped again because the host may still be editing it in place.
             * Any prefix change falls back to a full inspection and capture.
             */
            const prefix = previousWireCache?.rawHistory;
            let verified: CapturedHistory | undefined;
            let messageWireBytes: number[] = [];
            if (previousWireCache && prefix) {
                messageWireBytes = inspect(prefix.count);
                verified = captureHistory(target, lease, prefix);
                for (let index = 0; verified && index < prefix.count; index += 1)
                    messageWireBytes[index] = previousWireCache.wireBytes[index] ?? 0;
            }
            if (!verified) messageWireBytes = inspect(0);
            const captured = verified ?? captureHistory(target, lease);
            inputCount = messageWireBytes.length;
            // Later reads use the captured members; the live array is only rechecked against them.
            const messages = captured.members as MessageLike[];
            logStage(
                sessionId,
                "prefixGuard",
                prefixGuardStartedAt,
                timings,
                `phase=capture verified=${verified?.verified?.count ?? 0}`,
            );
            const recheckCapture = (phase: string): void => {
                assertCurrentPass();
                const startedAt = performance.now();
                const unchanged =
                    readOwnDataProperty(output, "messages") === target &&
                    capturedMessagesUnchanged(target, captured);
                logStage(sessionId, "prefixGuard", startedAt, timings, `phase=${phase}`);
                if (!unchanged) throw new PassDeclined(sessionId, "source_changed", phase);
            };
            // The delta decision and the charges it implies derive from the capture, so byte pressure declines before the first await.
            if (previousWireCache) failOpenSource = { captured, previous: previousWireCache };
            let wireDelta =
                !state.forceFullWire && previousWireCache
                    ? computeWireDelta(previousWireCache, captured)
                    : undefined;
            const reserveWire = (from: number, to: number): void => {
                let bytes = 0;
                for (let index = from; index < to; index += 1)
                    bytes += WIRE_PROJECTION_FACTOR * (messageWireBytes[index] ?? 0);
                charge(bytes, "wire projection");
            };
            reserveWire(wireDelta?.rawStart ?? 0, messages.length);
            let memoCopyBytes = 0;
            for (const id of state.ordinals.entries.keys())
                memoCopyBytes += ORDINAL_ENTRY_RETAINED_BYTES + id.length * 2;
            charge(memoCopyBytes, "ordinal memo copy");
            const syntheticTurn = observeSyntheticTurn(state, messages);
            if (syntheticTurn && state.syntheticTurnCount >= 3 && !state.syntheticCascadeLogged) {
                state.syntheticCascadeLogged = true;
                sessionLog.warn(
                    sessionId,
                    `rust synthetic-turn cascade: ${state.syntheticTurnCount} consecutive synthetic user turns with no real user message`,
                );
            }
            const passUsageSnapshot = loadContextUsage(deps, sessionId);
            let model = modelFromMessages(messages);
            // Both verdicts freeze from the first user message in the live array before the DB is consulted; a session whose first user row is not yet persisted otherwise reads as provisional and fails closed.
            resolveEidnaraReduceAvailabilityFromMessages(sessionId, messages);
            const reduceAvailability = resolveEidnaraReduceAvailability(sessionId);
            resolveTodowriteAvailabilityFromMessages(sessionId, messages);
            const todoAvailability = resolveTodowriteAvailability(sessionId);
            const toolPresent = reduceAvailability.frozen && reduceAvailability.callable;
            const activeAgent = activeAgentFromMessages(messages);
            // Ordinal work stages in a charged pass-local copy of the memo; only accepted publication promotes it.
            const stagedMemo: ModuleOrdinalMemo = {
                ...state.ordinals,
                entries: new Map(state.ordinals.entries),
            };
            const provisionalBase = wireDelta
                ? (() => {
                      for (let index = wireDelta.rawStart - 1; index >= 0; index -= 1) {
                          const priorId = messageIdOf(messages[index]);
                          if (!priorId) continue;
                          const prior = stagedMemo.entries.get(priorId);
                          if (prior !== undefined)
                              return Math.max(prior, stagedMemo.continuationBase ?? 0);
                      }
                      return Math.max(
                          stagedMemo.canonicalCount ?? 0,
                          stagedMemo.continuationBase ?? 0,
                      );
                  })()
                : stagedMemo.continuationBase;
            // Every message read above is synchronous; the awaits below read nothing from the source, so the next recheck precedes ordinal annotation.
            // The directory read also records a host-reported `parentID`, so it runs before the subagent classification is read.
            const directory = await resolveSessionDirectory(deps, sessionId);
            if (deps.isSessionDeleted?.(sessionId)) {
                deps.onSessionDeletedDuringPreflight?.(sessionId);
                throw new PassDeclined(sessionId, "deleted");
            }
            if (deps.isInternalChildSession?.(sessionId)) {
                throw new PassDeclined(sessionId, "internal_child");
            }
            const isSubagent = deps.isSubagentSession(sessionId);
            const systemPromptHash = deps.systemPromptHashFor(sessionId);
            let preflightError: unknown;
            if (!model) {
                try {
                    model = findLastAssistantModelFromOpenCodeDb(sessionId) ?? undefined;
                } catch (error) {
                    preflightError = error;
                }
            }
            const modelKey = model
                ? piModelRefToCanonical(resolveModelKey(model.providerID, model.modelID) ?? "")
                : null;
            let resolvedContextLimit: number | undefined;
            let resolvedWindowGeometry: WindowGeometryResult | undefined;
            if (model) {
                try {
                    resolvedContextLimit = resolveTrustedContextLimit(
                        model.providerID,
                        model.modelID,
                    );
                    resolvedWindowGeometry = resolveContextWindowGeometry(
                        model.providerID,
                        model.modelID,
                    );
                } catch (error) {
                    preflightError ??= error;
                }
            }
            const transformGeometry = transformGeometryForWire(resolvedWindowGeometry);
            assertCurrentPass();
            // Pass the module one bool combining the frozen map verdict and OpenCode's live permission decision.
            // Synthesis fails closed when host evidence is provisional or missing.
            const todoToolPresent = await resolveCombinedTodowriteVerdict(
                deps,
                sessionId,
                activeAgent,
                todoAvailability,
            );
            assertCurrentPass();
            if (preflightError) throw preflightError;
            const usage = passUsageSnapshot;
            // The usage sample's percentage was computed against `resolveContextLimit`, which substitutes the 128k default for a model models.dev cannot name, so inverting it recovers that default rather than a host report.
            const reportedContextLimit =
                resolvedContextLimit && resolvedContextLimit > 0 ? resolvedContextLimit : undefined;
            const contextLimit =
                reportedContextLimit ??
                (usage && usage.percentage > 0
                    ? Math.round(usage.inputTokens / (usage.percentage / 100))
                    : 128_000);
            const threshold = resolveExecuteThreshold(
                deps.executeThresholdPercentage ?? 65,
                modelKey ?? undefined,
                65,
                { tokensConfig: deps.executeThresholdTokens, contextLimit },
            );
            const historyBudgetTokens = resolveHistoryBudgetTokens(
                deps.historyBudgetPercentage,
                usage ?? { percentage: 0, inputTokens: 0 },
                deps.executeThresholdPercentage,
                modelKey ?? undefined,
                deps.executeThresholdTokens,
                resolvedContextLimit,
            );
            const midTurn = isMidTurn(deps, sessionId);
            const requestObservedAtMs = Date.now();
            const passInputs: Record<string, unknown> = {
                effective_execute_threshold: threshold,
                auto_search_enabled: deps.autoSearch?.enabled ?? true,
                auto_search_score_threshold: deps.autoSearch?.scoreThreshold ?? 0.6,
                auto_search_min_prompt_chars: deps.autoSearch?.minPromptChars ?? 20,
                history_budget_tokens: historyBudgetTokens,
                clear_reasoning_age: deps.clearReasoningAge,
                terse_text_compression_enabled:
                    !isSubagent && deps.terse_text_compressionTextCompression?.enabled === true,
                terse_text_compression_min_chars:
                    deps.terse_text_compressionTextCompression?.minChars ?? 500,
                cache_ttl: resolveCacheTtl(deps.cacheTtl, modelKey ?? undefined),
                is_subagent: isSubagent,
                tool_present: toolPresent,
                todo_tool_present: todoToolPresent,
                ...promptSurfaceWireFields(deps.promptSurfaceRuntime, deps.promptSurface, modelKey),
                protected_tags: deps.protectedTags ?? DEFAULT_PROTECTED_TAGS,
            };
            /**
             * Prime the memo asynchronously, recheck the captured messages, then annotate them
             * synchronously so no message read follows an await without a fresh guard.
             */
            const resolveOrdinals = async (
                rawStart: number,
                base: number | undefined,
                detail: string,
            ): Promise<OrdinalResolution> => {
                // Annotated shells and their memo entries coexist with the staged copy.
                charge(
                    (messages.length - rawStart) * ORDINAL_ENTRY_RETAINED_BYTES * 2,
                    "ordinal annotation",
                );
                const inputMessages = rawStart === 0 ? messages : messages.slice(rawStart);
                const startedAt = performance.now();
                const primed = await primeOrdinalMemo({
                    sessionId,
                    memo: stagedMemo,
                    budget: {
                        signal: lease.signal,
                        reserve: (bytes) => charge(bytes, `ordinal scan for session ${sessionId}`),
                    },
                });
                recheckCapture(`ordinal:${detail}`);
                if (!primed.ok) {
                    logStage(sessionId, "ordinalResolve", startedAt, timings, detail);
                    return primed;
                }
                const resolved = annotateOrdinals({
                    messages: inputMessages,
                    memo: stagedMemo,
                    primed: primed.primed,
                    provisionalBase: base,
                });
                logStage(sessionId, "ordinalResolve", startedAt, timings, detail);
                if (resolved.ok) {
                    stagedMemo.memoGeneration = stagedMemo.generation;
                    stagedMemo.anchor = primed.primed.memoAnchor;
                    stagedMemo.storedCount = primed.primed.memoStoredCount;
                    stagedMemo.canonicalCount = primed.primed.memoCanonicalCount;
                }
                return resolved;
            };
            let resolved = await resolveOrdinals(
                wireDelta?.rawStart ?? 0,
                provisionalBase,
                "attempt=first",
            );
            if (!resolved.ok) {
                if (wireDelta) reserveWire(0, wireDelta.rawStart);
                wireDelta = undefined;
                // A memo generation the scan cannot match forces a full re-prime without an anchor.
                stagedMemo.memoGeneration = -1;
                resolved = await resolveOrdinals(
                    0,
                    stagedMemo.continuationBase,
                    "fallback=clean_full",
                );
            }
            if (!resolved.ok) {
                throw new Error(
                    `rust ordinal ${resolved.reason}: messageId=${resolved.messageId ?? "unknown"} ` +
                        `index=${resolved.messageIndex ?? "unknown"} role=${resolved.messageRole ?? "unknown"}`,
                );
            }

            recheckCapture("wire-build");
            const projectRoot = options.projectRoot ?? directory;
            state.routeRoot = projectRoot;
            deliveries.projectRoot = projectRoot;
            const wireBuildStartedAt = performance.now();
            const encodedInput = encodeOpenCodeMessagesToCk(resolved.annotatedInput);
            timings.wireMessages = messages.length - (wireDelta?.rawStart ?? 0);
            charge(messages.length * LENGTH_SLOT_BYTES, "input lengths");
            const inputLengths = measureInputLengths(
                messages,
                previousWireCache,
                wireDelta?.rawStart ?? 0,
            );
            let pendingWireCache: RustWireCache =
                // An empty delta replaces nothing: the terminal message, its wire visibility, and both before-last fingerprints stay the acknowledged ones.
                wireDelta && previousWireCache && wireDelta.rawStart === messages.length
                    ? { ...previousWireCache, applied: undefined }
                    : buildWireCache({
                          messages,
                          encoded: encodedInput,
                          captured,
                          wireBytes: messageWireBytes,
                          inputLengths,
                          delta: wireDelta,
                      });
            // The retained output stays reusable only while the cache that applied it survives.
            let previousApplied = previousWireCache?.applied;
            if (
                previousApplied &&
                !capturedMessagesUnchanged(previousApplied.values, previousApplied.capture)
            ) {
                previousApplied = undefined;
                if (previousWireCache) previousWireCache.applied = undefined;
                appliedOutputs.release(sessionId);
            }
            let baseRevision = nextBaseRevision();
            const usageEntry = deps.contextUsageMap.get(sessionId);
            // Fields both the first attempt and the full-wire retry forward
            // unchanged. `fullArrayFingerprint` stays per-site: the retry
            // rebuilds `pendingWireCache` before its call and must read the
            // rebuilt fingerprint.
            const transformBodyBase = {
                sessionId,
                passInputs,
                // The daemon keeps its persisted usage when the request carries none; a zero sample with a nonzero limit would replace it.
                usage: usage ? passUsage(usage, contextLimit) : undefined,
                prevResponseCacheUsage: prevResponseCacheUsage(usage),
                geometry: transformGeometry,
                modelKey: modelKey ?? null,
                providerId: model?.providerID ?? null,
                systemPromptHash,
                midTurn,
                prevResponseCompletedAtMs:
                    usageEntry?.lastResponseTime !== undefined && usageEntry.lastResponseTime > 0
                        ? usageEntry.lastResponseTime
                        : undefined,
                requestObservedAtMs,
            };
            let body = buildTransformBody({
                ...transformBodyBase,
                baseRevision,
                previousOutputRevision: previousApplied?.revision,
                input: encodedInput,
                nativeMessages: wireDelta ? messages.slice(wireDelta.rawStart) : messages,
                fullArrayFingerprint: pendingWireCache.fingerprint,
                tailDelta: wireDelta
                    ? {
                          after: wireDelta.after,
                          replaceFrom: wireDelta.wireStart,
                          nativeReplaceFrom: wireDelta.rawStart,
                      }
                    : undefined,
            });
            logStage(
                sessionId,
                "wireBuild",
                wireBuildStartedAt,
                timings,
                wireDelta ? `mode=tail_delta input=${encodedInput.length}` : "mode=full",
            );
            type TransformSeriesRestart = {
                reason: "attempt_mismatch" | "reconnect";
                pages: number;
                atPage: number;
            };
            type TransformSeriesResult =
                | { response: Record<string, unknown> }
                | { restart: TransformSeriesRestart };
            /** Each series freezes its pages from revalidated source values. */
            const sendTransformSeries = async (
                payload: Record<string, unknown>,
                detail: string,
            ): Promise<TransformSeriesResult> => {
                const series = buildPagedModuleTransformPayloads(payload);
                const paged = series.some(
                    (entry) => typeof entry.page.transform_page_id === "string",
                );
                let response: Record<string, unknown> | undefined;
                for (const [index, { page, bytes }] of series.entries()) {
                    const restart = (
                        reason: TransformSeriesRestart["reason"],
                    ): TransformSeriesResult => ({
                        restart: { reason, pages: series.length, atPage: index },
                    });
                    // A `session.deleted` that landed during preflight or an earlier page has already queued the daemon-side delete; sending now would recreate the session's durable state.
                    // Page bodies are frozen text, so a source walk here cannot change what is sent; the recheck before publication covers the series.
                    assertCurrentPass();
                    const transportStartedAt = performance.now();
                    let moduleResponse: unknown;
                    try {
                        moduleResponse = await options.moduleClient.call({
                            sessionId,
                            projectRoot,
                            method: "transform",
                            body: page,
                            signal: lease.signal,
                            generationSensitive: paged && index > 0,
                        });
                    } catch (error) {
                        if (paged && isTransformPageAttemptMismatch(error))
                            return restart("attempt_mismatch");
                        throw error;
                    }
                    if (paged && isModuleTransportGenerationChangedResult(moduleResponse))
                        return restart("reconnect");
                    if (paged && isTransformPageAttemptMismatch(moduleResponse))
                        return restart("attempt_mismatch");
                    response = responseValue(moduleResponse);
                    for (const id of noteDeliveryPassIds(response)) deliveries.attempted.add(id);
                    timings.transportBytes += bytes;
                    timings.transportPages += 1;
                    logStage(
                        sessionId,
                        "transport",
                        transportStartedAt,
                        timings,
                        `page=${index + 1}/${series.length}${detail}`,
                    );
                }
                if (!response) throw new Error("rust module returned no transform response");
                // The daemon's session lane already holds an active and a waiting pass for this session.
                if (response.status === "session_busy") {
                    throw new PassDeclined(sessionId, "daemon_session_busy");
                }
                return { response };
            };
            let transformSeriesRestarted = false;
            // One bounded series restart is the only permitted page-level recovery; it is a fresh attempt over the same validated capture.
            const sendTransformSeriesWithSingleRestart = async (
                payload: Record<string, unknown>,
                detail: string,
            ): Promise<Record<string, unknown>> => {
                let result = await sendTransformSeries(payload, detail);
                if (!("restart" in result)) return result.response;
                if (transformSeriesRestarted) {
                    throw new Error(
                        `rust transform page series restart exhausted: reason=${result.restart.reason}`,
                    );
                }
                transformSeriesRestarted = true;
                sessionLog.warn(
                    sessionId,
                    `transform_series_restart reason=${result.restart.reason} pages=${result.restart.pages} at_page=${result.restart.atPage}`,
                );
                recheckCapture("series-restart");
                result = await sendTransformSeries(payload, `${detail} restart=series`);
                if ("restart" in result) {
                    throw new Error(
                        `rust transform page series restart exhausted: reason=${result.restart.reason}`,
                    );
                }
                return result.response;
            };
            // The daemon commits its native-output snapshot on response, so a pass that exits after dispatch without committing its cache must resend the full history.
            state.forceFullWire = true;
            let response = await sendTransformSeriesWithSingleRestart(body, "");
            captureResponseTelemetry(response);
            if (isNeedFullSync(response)) {
                // A cleared or superseded session must not receive the flag.
                assertCurrentPass();
                state.forceFullWire = true;
                // The retry names a fresh input snapshot; the recipe it receives binds to that one.
                baseRevision = nextBaseRevision();
                if (wireDelta) {
                    reserveWire(0, wireDelta.rawStart);
                    let retryResolved = await resolveOrdinals(
                        0,
                        stagedMemo.continuationBase,
                        "retry=full",
                    );
                    if (!retryResolved.ok) {
                        stagedMemo.memoGeneration = -1;
                        retryResolved = await resolveOrdinals(
                            0,
                            undefined,
                            "retry=full fallback=clean_full",
                        );
                    }
                    if (!retryResolved.ok) {
                        throw new Error(`rust ordinal ${retryResolved.reason} during full retry`);
                    }
                    recheckCapture("retry-wire-build");
                    const retryEncodedInput = encodeOpenCodeMessagesToCk(
                        retryResolved.annotatedInput,
                    );
                    timings.wireMessages = messages.length;
                    pendingWireCache = buildWireCache({
                        messages,
                        encoded: retryEncodedInput,
                        captured,
                        wireBytes: messageWireBytes,
                        inputLengths,
                    });
                    const retryWireBuildStartedAt = performance.now();
                    body = buildTransformBody({
                        ...transformBodyBase,
                        baseRevision,
                        previousOutputRevision: previousApplied?.revision,
                        input: retryEncodedInput,
                        nativeMessages: messages,
                        fullArrayFingerprint: pendingWireCache.fingerprint,
                    });
                    logStage(
                        sessionId,
                        "wireBuild",
                        retryWireBuildStartedAt,
                        timings,
                        "retry=full",
                    );
                } else {
                    // The same body is serialized again from the live objects, so the source is rechecked first.
                    recheckCapture("full-retry");
                    body = { ...body, base_revision: baseRevision };
                }
                response = await sendTransformSeriesWithSingleRestart(body, " retry=full");
                captureResponseTelemetry(response);
                if (isNeedFullSync(response)) {
                    throw new Error("rust module still requires full sync after a full-array send");
                }
            }
            const appliedDeliveryPassIds = new Set(noteDeliveryPassIds(response));
            const applyStartedAt = performance.now();
            try {
                // Previous outputs may share objects with an earlier host array, not this pass's input.
                if (
                    previousApplied &&
                    response.previous_output_revision !== undefined &&
                    !capturedMessagesUnchanged(previousApplied.values, previousApplied.capture)
                ) {
                    if (previousWireCache) previousWireCache.applied = undefined;
                    appliedOutputs.release(sessionId);
                    throw new PassDeclined(sessionId, "source_changed", "previous output");
                }
                // Recipe validation, sizing, and every boundary check run before the host array is touched.
                const application = applyTransformRecipe(
                    response,
                    { revision: baseRevision, values: messages, lengths: inputLengths },
                    previousApplied,
                    (slots, insertedSlots) =>
                        lease.reserve(
                            slots * (CANDIDATE_SLOT_BYTES + LENGTH_SLOT_BYTES) +
                                insertedSlots * LENGTH_SLOT_BYTES,
                        ),
                );
                const candidate = application.values;
                let applied: AppliedOutput | undefined;
                try {
                    const inspection = inspectReferenceableMessages(
                        candidate,
                        lease.remainingBytes,
                    );
                    if (inspection.ok && lease.reserve(inspection.estimatedBytes)) {
                        applied = {
                            revision: application.outputRevision,
                            values: candidate,
                            lengths: application.lengths,
                            capture: captureMessages(candidate, lease),
                            charge:
                                application.bytes +
                                application.lengths.length * LENGTH_SLOT_BYTES +
                                inspection.estimatedBytes,
                        };
                    }
                } catch (error) {
                    if (!(error instanceof CaptureBudgetExceeded)) throw error;
                }
                const boundaryId = response.boundary_id;
                if (typeof boundaryId === "string" && boundaryId.length > 0) {
                    assertNativeBoundary(candidate, sessionId, boundaryId);
                }
                const ordinalContinuationBase = response.ordinal_continuation_base;
                if (
                    typeof ordinalContinuationBase === "number" &&
                    Number.isSafeInteger(ordinalContinuationBase) &&
                    ordinalContinuationBase > 0
                ) {
                    if (stagedMemo.continuationBase === undefined) {
                        for (const [messageId, ordinal] of stagedMemo.entries) {
                            const shifted = ordinal + ordinalContinuationBase;
                            if (!Number.isSafeInteger(shifted))
                                throw new Error("ordinal continuation overflow");
                            stagedMemo.entries.set(messageId, shifted);
                        }
                        const count = (stagedMemo.canonicalCount ?? 0) + ordinalContinuationBase;
                        if (!Number.isSafeInteger(count))
                            throw new Error("ordinal continuation overflow");
                        stagedMemo.canonicalCount = count;
                    }
                    stagedMemo.continuationBase = ordinalContinuationBase;
                }
                // Final synchronous guards: ownership, wire invalidation, source membership and content, and the host container contract.
                recheckCapture("publish");
                const publishRejection = hostArrayReplacementRejection(target);
                if (publishRejection !== null) {
                    throw new PassDeclined(sessionId, "host_container", publishRejection);
                }
                // Every candidate entry's canonical length is charged, not the inserted payload alone.
                const invocation = validateInvocation(application.lengths, inputLengths, {
                    maxTokens: reportedContextLimit,
                    headroomPermille: INVOCATION_HEADROOM_PERMILLE,
                    profile: "opencode-heuristic",
                });
                if (!invocation.ok) {
                    throw new PassDeclined(
                        sessionId,
                        "invocation_budget",
                        `${invocation.candidate.chargedTokens} charged tokens over ${invocation.limit}, growing from ${invocation.incoming.bytes} to ${invocation.candidate.bytes} bytes, under ${invocation.candidate.profile.identity} ${invocation.candidate.profile.revision}`,
                        "warn",
                    );
                }
                logStage(sessionId, "apply", applyStartedAt, timings);
                const applyReplaceStartedAt = performance.now();
                // Publication and state promotion are synchronous from here to the lease release.
                replaceHostArrayContents(target, candidate);
                // A refused retention keeps the pass; only the next pass loses its `previous` source.
                appliedOutputs.release(sessionId);
                pendingWireCache.applied =
                    applied && appliedOutputs.retain(sessionId, applied.charge)
                        ? applied
                        : undefined;
                state.ordinals = stagedMemo;
                state.initialized = true;
                state.consecutiveFailures = 0;
                state.forceFullWire = false;
                storeWireCache(sessionId, pendingWireCache);
                deliveries.applied = appliedDeliveryPassIds;
                logStage(sessionId, "apply", applyReplaceStartedAt, timings);
            } catch (error) {
                logStage(sessionId, "apply", applyStartedAt, timings, "failed=true");
                throw error;
            }
            appliedAt = performance.now();
            finishPass(true);
        } catch (caught) {
            servedFrom = "raw";
            materializeReason = "none";
            const error =
                caught instanceof CaptureBudgetExceeded
                    ? new PassDeclined(sessionId, "capture_bytes", caught.message)
                    : caught;
            if (error instanceof PassDeclined || lease.signal.aborted) {
                decision = error instanceof PassDeclined ? `declined:${error.reason}` : "cancelled";
                sessionLog[error instanceof PassDeclined ? error.logLevel : "debug"](
                    sessionId,
                    error instanceof Error ? error.message : String(error),
                );
                // Byte pressure recurs on every pass, so serving raw would send the whole history.
                if (
                    error instanceof PassDeclined &&
                    error.reason === "capture_bytes" &&
                    serveLastApplied()
                )
                    servedFrom = "last_applied";
            } else {
                if (decision.toLowerCase() !== "need_full_sync") decision = "error";
                const servedLastApplied = serveLastApplied();
                if (servedLastApplied) servedFrom = "last_applied";
                markFailure(sessionId, state, error, servedLastApplied);
            }
            finishPass(false);
        }
        return deliveries;
    };

    const run = (sessionId: string, output: { messages: unknown[] }): Promise<void> => {
        const admission = captureAdmission.admit(sessionId);
        if ("declined" in admission) {
            // Global count exhaustion is a saturation signal; a same-session supersession is routine.
            sessionLog[admission.declined === "pass_count" ? "warn" : "debug"](
                sessionId,
                `rust transform declined before dispatch: ${admission.declined}`,
            );
            return Promise.resolve();
        }
        const lease = admission.lease;
        // execute settles before admission is released; only route metadata reaches delivery.
        return execute(sessionId, output, lease)
            .finally(lease.release.bind(lease))
            .then(deliverTransformNotes.bind(undefined, options.moduleClient));
    };

    return {
        run,
        clearSession(sessionId: string): void {
            // Without a `routeRoot`, the fallback is the root `run` would have used, so the daemon's durable state is still addressed.
            const projectRoot =
                states.get(sessionId)?.routeRoot ??
                options.projectRoot ??
                knownSessionDirectory(deps, sessionId);
            states.delete(sessionId);
            releaseWireCache(sessionId);
            captureAdmission.requestCancel(sessionId, `rust session ${sessionId} cleared`);
            // Route close asks the host to settle active work within its close budget before deletion acquires the lane; cleanup closes the replacement route.
            options.moduleClient.closeSession?.(sessionId);
            if (options.moduleClient.deleteSession) {
                void options.moduleClient
                    .deleteSession(sessionId, projectRoot)
                    .catch((error) => {
                        sessionLog.warn(sessionId, "rust module session deletion failed:", error);
                    })
                    .finally(() => options.moduleClient.closeSession?.(sessionId));
            }
        },
        invalidateWireState,
        getState(sessionId: string): Readonly<RustSessionState> {
            const state = ensureState(states, sessionId);
            return {
                ...state,
                ordinals: { ...state.ordinals, entries: new Map(state.ordinals.entries) },
            };
        },
    };
}

export const __rustModeTransformTest = {
    WIRE_CACHE_SESSION_CAPACITY,
    OPTIONAL_OUTPUT_BUDGET_BYTES,
    AppliedOutputBudget,
    applyTransformRecipe,
    buildTransformBody,
    transformGeometryForWire,
    formatRustPassLog,
    isTransformPageAttemptMismatch,
    createRustModeTransform,
};
