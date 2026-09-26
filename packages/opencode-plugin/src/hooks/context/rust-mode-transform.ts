import { randomUUID } from "node:crypto";

import { DEFAULT_PROTECTED_TAGS } from "../../features/context/defaults";
import type { BoundedSessionMap } from "../../shared/bounded-session-map";
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
    inspectReferenceableMessages,
    publicationRejection,
    publishInPlace,
    readOwnDataProperty,
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

/** Retained outputs are bounded to the sessions one OpenCode process keeps active. An evicted session captures its next pass in full and has no `previous` source. */
const RETAINED_OUTPUT_SESSION_CAPACITY = 64;
/** Retained outputs are optional reuse state, budgeted apart from the frame and reconstruction caps. */
const RETAINED_OUTPUT_BUDGET_BYTES = 64 * 1024 * 1024;
/** Each retained canonical length occupies one number slot. */
const LENGTH_SLOT_BYTES = 8;
/** Each retained message keeps an input length and a wire bound; the history digest is fixed size. */
const HISTORY_ENTRY_RETAINED_BYTES = 2 * LENGTH_SLOT_BYTES;

/** One successfully applied output, eligible as the `previous` source of the next recipe. */
interface AppliedOutput {
    revision: string;
    values: readonly unknown[];
    lengths: readonly number[];
    capture: CapturedMessages;
    /** Canonical bytes, retained lengths, and snapshots; part of the record's charge. */
    charge: number;
}

/**
 * A session's retained output: the capture basis of the input its last published pass submitted,
 * which lets the next capture verify that prefix instead of taping it again and lets a failed pass
 * decide whether the applied output is still served, and the output that pass applied.
 */
interface RetainedOutput {
    readonly rawCount: number;
    /** Digest of every submitted message but the last. */
    readonly rawHistory: HistoryDigest;
    /** The former terminal alone; the host may edit it in place, so it is compared separately. */
    readonly rawTerminal?: HistoryDigest;
    /** Inspection wire bounds of the submitted messages, reused for the verified prefix. */
    readonly wireBytes: readonly number[];
    /** Canonical JSON length of each submitted native message, reused for the verified prefix. */
    readonly inputLengths: readonly number[];
    /** Cleared only by `RetainedOutputs`, which owns the charge. */
    readonly applied?: AppliedOutput;
    /** One charge for the whole record, the applied output's included. */
    readonly charge: number;
}

/** The fields `RetainedOutputs` alone may change, keeping `used` equal to the summed charges. */
type OwnedRetainedOutput = { applied?: AppliedOutput; charge: number };

/**
 * Holds one retained output per session under a session-count and a byte limit, evicting the
 * least recently used record first; a record is used when it is retained or read with `get`.
 * Failed passes serving the last applied output refresh its session. Eviction never releases a
 * capture lease.
 */
class RetainedOutputs {
    private readonly records = new Map<string, RetainedOutput>();
    private used = 0;

    constructor(
        private readonly maxSessions: number,
        private readonly maxBytes: number,
    ) {}

    /** Returns the record and moves it to the back of the eviction order. */
    get(sessionId: string): RetainedOutput | undefined {
        const record = this.records.get(sessionId);
        if (record === undefined) return undefined;
        this.records.delete(sessionId);
        this.records.set(sessionId, record);
        return record;
    }

    /** Returns the record without changing the eviction order. */
    peek(sessionId: string): RetainedOutput | undefined {
        return this.records.get(sessionId);
    }

    /** Retains `record`; one over the whole budget is retained without its applied output, else refused. */
    retain(sessionId: string, record: RetainedOutput): boolean {
        this.release(sessionId);
        if (record.charge > this.maxBytes) {
            if (!record.applied || record.charge - record.applied.charge > this.maxBytes)
                return false;
            this.clearApplied(record);
        }
        for (const [oldest] of this.records) {
            if (this.records.size < this.maxSessions && this.used + record.charge <= this.maxBytes)
                break;
            this.release(oldest);
        }
        this.records.set(sessionId, record);
        this.used += record.charge;
        return true;
    }

    /** Keeps the record's basis and gives back the applied output's share of the charge. */
    dropApplied(sessionId: string, record: RetainedOutput): void {
        if (!record.applied || this.records.get(sessionId) !== record) return;
        this.used -= record.applied.charge;
        this.clearApplied(record);
    }

    release(sessionId: string): void {
        const record = this.records.get(sessionId);
        if (record === undefined) return;
        this.records.delete(sessionId);
        this.used -= record.charge;
    }

    private clearApplied(record: RetainedOutput): void {
        const owned = record as OwnedRetainedOutput;
        owned.charge -= owned.applied?.charge ?? 0;
        owned.applied = undefined;
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

export interface RustSessionState {
    initialized: boolean;
    consecutiveFailures: number;
    passCount: number;
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
    /** Retained-output budget across sessions; tests inject a smaller one. */
    retainedOutputBudgetBytes?: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object";
}

function messageInfo(value: unknown): Record<string, unknown> {
    if (!isRecord(value)) return {};
    return isRecord(value.info) ? value.info : value;
}

/**
 * A failed pass may serve the retained output only when every message the previous pass submitted,
 * its terminal included, is unchanged and in place. New messages may follow; an edit, removal,
 * revert, or reorder of an acknowledged message leaves the retained output stale.
 */
function isAppendOnlyExtension(previous: RetainedOutput, captured: CapturedHistory): boolean {
    return (
        captured.members.length >= previous.rawCount &&
        captured.verified !== undefined &&
        historyDigestsEqual(captured.verified, previous.rawHistory) &&
        (previous.rawCount === 0 || formerTerminalUnchanged(previous, captured))
    );
}

/** A verified capture tapes the former terminal first, so its boundary is that member. */
function formerTerminalUnchanged(previous: RetainedOutput, captured: CapturedHistory): boolean {
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
function appliedOutputGrew(previous: RetainedOutput, applied: AppliedOutput): boolean {
    let appliedBytes = 0;
    for (const length of applied.lengths) appliedBytes += length;
    let prefixBytes = 0;
    for (const length of previous.inputLengths) prefixBytes += length;
    return appliedBytes > prefixBytes;
}

/**
 * Lengths for the submitted native array: members the capture verified against the retained
 * digest keep the lengths measured when they were first sent, and only the rest are measured.
 */
function measureInputLengths(
    messages: readonly unknown[],
    previous: RetainedOutput | undefined,
    verifiedCount: number,
): number[] {
    const lengths = previous ? previous.inputLengths.slice(0, verifiedCount) : [];
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
    | "publication_failed"
    | "deleted"
    | "internal_child"
    | "daemon_session_busy"
    | "daemon_status_unrecognized";

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
    invalidateOrdinals: (sessionId: string) => void;
    getState: (sessionId: string) => Readonly<RustSessionState>;
} {
    const states = new Map<string, RustSessionState>();
    const retainedOutputs = new RetainedOutputs(
        RETAINED_OUTPUT_SESSION_CAPACITY,
        options.retainedOutputBudgetBytes ?? RETAINED_OUTPUT_BUDGET_BYTES,
    );
    /** A digest keeps each symbol, and so its description, alive with the record. */
    const retainedSymbolBytes = (digest: HistoryDigest | undefined): number => {
        let bytes = 0;
        for (const symbol of digest?.symbols ?? [])
            bytes += CANDIDATE_SLOT_BYTES + (symbol.description?.length ?? 0) * 2;
        return bytes;
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

    /** A removed message can hold a memo ordinal; a pass in flight is cancelled so it cannot promote the stale memo it copied. */
    const invalidateOrdinals = (sessionId: string): void => {
        captureAdmission.requestCancel(sessionId, `rust session ${sessionId} ordinals invalidated`);
        const state = states.get(sessionId);
        if (!state) return;
        state.ordinals = {
            ...state.ordinals,
            entries: new Map(),
            anchor: null,
            storedCount: null,
            canonicalCount: 0,
        };
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
        const hostRejection = publicationRejection(target, 0);
        if (hostRejection !== null) {
            sessionLog.debug(
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
                        `request_to_handler=${stage("request_observed_to_handler")} ` +
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
        // Clearing a session replaces its state object; supersession, clearing, and ordinal invalidation abort the capture lease.
        const assertCurrentPass = (): void => {
            if (states.get(sessionId) !== state) throw new PassDeclined(sessionId, "cleared");
            if (lease.signal.aborted) throw new PassDeclined(sessionId, "superseded");
        };
        const charge = (bytes: number, detail: string): void => {
            if (!lease.reserve(bytes)) throw new CaptureBudgetExceeded(detail);
        };
        let failOpenSource: { captured: CapturedHistory; previous: RetainedOutput } | undefined;
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
                    lease.signal.aborted ||
                    retainedOutputs.peek(sessionId) !== source.previous ||
                    !isAppendOnlyExtension(source.previous, source.captured) ||
                    appliedOutputGrew(source.previous, applied) ||
                    readOwnDataProperty(output, "messages") !== target ||
                    !capturedMessagesUnchanged(target, source.captured)
                )
                    return false;
                if (!capturedMessagesUnchanged(applied.values, applied.capture)) {
                    retainedOutputs.dropApplied(sessionId, source.previous);
                    return false;
                }
                const served = [
                    ...applied.values,
                    ...source.captured.members.slice(source.previous.rawCount),
                ];
                if (
                    publicationRejection(target, served.length) !== null ||
                    !lease.reserve(served.length * CANDIDATE_SLOT_BYTES)
                )
                    return false;
                const failure = publishInPlace(target, served, source.captured.members);
                if (failure) {
                    sessionLog.warn(
                        sessionId,
                        `rust transform fail-open publication failed: ${failure.detail}`,
                    );
                    return false;
                }
                return true;
            } catch (error) {
                sessionLog.warn(sessionId, "rust transform fail-open reuse declined:", error);
                return false;
            }
        };
        try {
            // Source domain is validated synchronously before any message read.
            const prefixGuardStartedAt = performance.now();
            const previous = retainedOutputs.get(sessionId);
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
            const prefix = previous?.rawHistory;
            let verified: CapturedHistory | undefined;
            let messageWireBytes: number[] = [];
            if (previous && prefix) {
                messageWireBytes = inspect(prefix.count);
                verified = captureHistory(target, lease, prefix);
                for (let index = 0; verified && index < prefix.count; index += 1)
                    messageWireBytes[index] = previous.wireBytes[index] ?? 0;
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
            // The wire charge derives from the capture, so byte pressure declines before the first await.
            if (previous) failOpenSource = { captured, previous };
            let wireBytes = 0;
            for (let index = 0; index < messageWireBytes.length; index += 1)
                wireBytes += WIRE_PROJECTION_FACTOR * (messageWireBytes[index] ?? 0);
            charge(wireBytes, "wire projection");
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
            const resolveOrdinals = async (detail: string): Promise<OrdinalResolution> => {
                const base = stagedMemo.continuationBase;
                // Annotated shells and their memo entries coexist with the staged copy.
                charge(messages.length * ORDINAL_ENTRY_RETAINED_BYTES * 2, "ordinal annotation");
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
                    messages,
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
            let resolved = await resolveOrdinals("attempt=first");
            if (!resolved.ok) {
                // A memo generation the scan cannot match forces a full re-prime without an anchor.
                stagedMemo.memoGeneration = -1;
                resolved = await resolveOrdinals("fallback=clean_full");
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
            timings.wireMessages = messages.length;
            charge(messages.length * LENGTH_SLOT_BYTES, "input lengths");
            const inputLengths = measureInputLengths(
                messages,
                previous,
                captured.verified?.count ?? 0,
            );
            // The retained output stays reusable only while the record that applied it survives.
            let previousApplied = previous?.applied;
            if (
                previous &&
                previousApplied &&
                !capturedMessagesUnchanged(previousApplied.values, previousApplied.capture)
            ) {
                previousApplied = undefined;
                retainedOutputs.dropApplied(sessionId, previous);
            }
            const baseRevision = nextBaseRevision();
            const usageEntry = deps.contextUsageMap.get(sessionId);
            const body = buildTransformBody({
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
                baseRevision,
                previousOutputRevision: previousApplied?.revision,
                input: encodedInput,
                nativeMessages: messages,
            });
            logStage(sessionId, "wireBuild", wireBuildStartedAt, timings);
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
                // session_busy: the daemon's session lane already holds an active and a waiting pass for this session.
                // An unrecognized status is declined the same way (Section 7.10.1 of the wire protocol).
                const busy = response.status === "session_busy";
                if (busy || (response.status !== undefined && response.status !== "ok")) {
                    assertCurrentPass();
                    throw new PassDeclined(
                        sessionId,
                        busy ? "daemon_session_busy" : "daemon_status_unrecognized",
                    );
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
            const response = await sendTransformSeriesWithSingleRestart(body, "");
            captureResponseTelemetry(response);
            const appliedDeliveryPassIds = new Set(noteDeliveryPassIds(response));
            const applyStartedAt = performance.now();
            try {
                // Previous outputs may share objects with an earlier host array, not this pass's input.
                if (
                    previousApplied &&
                    response.previous_output_revision !== undefined &&
                    !capturedMessagesUnchanged(previousApplied.values, previousApplied.capture)
                ) {
                    if (previous) retainedOutputs.dropApplied(sessionId, previous);
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
                // Final synchronous guards: ownership, source membership and content, and the host container contract.
                recheckCapture("publish");
                const publishRejection = publicationRejection(target, candidate.length);
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
                const failure = publishInPlace(target, candidate, messages);
                if (failure)
                    throw new PassDeclined(sessionId, "publication_failed", failure.detail, "warn");
                const record: RetainedOutput = {
                    rawCount: messages.length,
                    rawHistory: captured.history,
                    ...(captured.terminal ? { rawTerminal: captured.terminal } : {}),
                    wireBytes: messageWireBytes,
                    inputLengths,
                    ...(applied ? { applied } : {}),
                    charge:
                        messages.length * HISTORY_ENTRY_RETAINED_BYTES +
                        retainedSymbolBytes(captured.history) +
                        retainedSymbolBytes(captured.terminal) +
                        (applied?.charge ?? 0),
                };
                // A refused retention keeps the pass; the next pass loses its verified prefix, or only its `previous` source.
                retainedOutputs.retain(sessionId, record);
                state.ordinals = stagedMemo;
                state.initialized = true;
                state.consecutiveFailures = 0;
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
                decision = "error";
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
            retainedOutputs.release(sessionId);
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
        invalidateOrdinals,
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
    RETAINED_OUTPUT_SESSION_CAPACITY,
    RETAINED_OUTPUT_BUDGET_BYTES,
    RetainedOutputs,
    applyTransformRecipe,
    buildTransformBody,
    transformGeometryForWire,
    formatRustPassLog,
    isTransformPageAttemptMismatch,
    createRustModeTransform,
};
