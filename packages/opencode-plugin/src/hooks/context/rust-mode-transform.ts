import { createHash } from "node:crypto";

import { DEFAULT_PROTECTED_TAGS } from "../../features/context/defaults";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { piModelRefToCanonical } from "../../shared/harness-provider-map";
import { sessionLog } from "../../shared/logger";
import {
    type PromptSurfaceConfig,
    promptSurfaceConfigIdentity,
    resolvePromptSurface,
} from "../../shared/prompt-surface";
import type { PromptSurfaceRuntime } from "../../shared/prompt-surface-runtime";
import type { WindowGeometryResult } from "../../shared/window-geometry";
import {
    resolveCtxReduceAvailability,
    resolveCtxReduceAvailabilityFromMessages,
    resolveTodowriteAvailability,
    resolveTodowriteAvailabilityFromMessages,
    type ToolAvailabilityVerdict,
    todowritePermissionDenied,
} from "./ctx-reduce-availability";
import type { ContextUsageEntry } from "./event-handler";
import type { ContextUsage } from "./event-payloads";
import {
    resolveCacheTtl,
    resolveContextWindowGeometry,
    resolveExecuteThreshold,
    resolveModelKey,
    resolveTrustedContextLimit,
} from "./event-resolvers";
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
    type CaptureLease,
    capturedMessagesUnchanged,
    captureMessages,
    defaultTransformCaptureAdmission,
    hostArrayReplacementRejection,
    inspectReferenceableMessages,
    type MessageContentSnapshot,
    readOwnDataProperty,
    replaceHostArrayContents,
    snapshotFieldsEqual,
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
    cavemanTextCompression?: { enabled: boolean; minChars: number };
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
    messages: readonly MessageLike[],
    availability: ToolAvailabilityVerdict,
): Promise<boolean> {
    if (!availability.frozen || !availability.callable || deps.compactionOff === true) return false;

    return !(await todowritePermissionDenied(
        deps.client,
        sessionId,
        activeAgentFromMessages(messages),
    ));
}

export interface RustModeModuleClient {
    call(args: {
        sessionId: string;
        projectRoot: string;
        method: ModuleMethod;
        body: unknown;
        signal?: AbortSignal;
        generationSensitive?: boolean;
    }): Promise<unknown>;
    deleteSession?(sessionId: string, projectRoot: string): Promise<void>;
    closeSession?(sessionId: string): void;
    hasSessionRoute?(sessionId: string): boolean;
}

/** Wire caches hold a session's content snapshots and its last native output, so the LRU bound is sized to the sessions one OpenCode process keeps active. An evicted session sends its next pass as a full array. */
const WIRE_CACHE_SESSION_CAPACITY = 64;

interface RustWireCache {
    rawCount: number;
    wireCount: number;
    rawLastVisible: boolean;
    /** Each pass re-verifies reused messages so in-place edits cannot reuse a stale prefix. */
    rawContentSnapshots: readonly MessageContentSnapshot[];
    ckFingerprint: string;
    ckPrefixFingerprintBeforeLast: string;
    nativeFingerprint: string;
    nativePrefixFingerprintBeforeLast: string;
    fingerprint: string;
    /** The previous acknowledged module output is reused by reference as the prefix for a validated native-output delta.
     * The array supplies the prefix for a validated native-output delta; eviction requires a full response. */
    nativeOutput?: unknown[];
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
 * Delta transport requires snapshots before the former terminal to equal
 * `previous.rawContentSnapshots`. The terminal is compared separately because an invisible
 * former terminal may have been edited in place while a message was appended.
 */
function computeWireDelta(
    previous: RustWireCache,
    snapshots: readonly MessageContentSnapshot[],
): WireDelta | undefined {
    if (snapshots.length < previous.rawCount) return undefined;
    const appending = snapshots.length > previous.rawCount;
    const formerTerminalIndex = previous.rawCount - 1;
    const prefixIntact =
        formerTerminalIndex <= previous.rawContentSnapshots.length &&
        snapshots
            .slice(0, Math.max(0, formerTerminalIndex))
            .every((snapshot, index) =>
                snapshotFieldsEqual(snapshot, previous.rawContentSnapshots[index]),
            );
    if (!prefixIntact) return undefined;
    const formerTerminalSnapshot = previous.rawContentSnapshots[formerTerminalIndex];
    const lastChanged =
        formerTerminalIndex >= 0 &&
        !(appending && previous.rawLastVisible) &&
        (formerTerminalSnapshot === undefined ||
            !snapshotFieldsEqual(snapshots[formerTerminalIndex], formerTerminalSnapshot));
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

/** The pending cache for a pass; `nativeOutput` is attached on publication. */
function buildWireCache(args: {
    messages: readonly MessageLike[];
    encoded: readonly { mid?: unknown }[];
    snapshots: readonly MessageContentSnapshot[];
    delta?: WireDelta;
}): RustWireCache {
    const { messages, encoded, snapshots, delta } = args;
    const rawLast = messages.at(-1);
    const rawLastId = rawLast === undefined ? null : messageIdOf(rawLast);
    const ck = buildWireFingerprint(encoded, delta?.ckAfter);
    const native = buildWireFingerprint(
        delta ? messages.slice(delta.rawStart) : messages,
        delta?.nativeAfter,
    );
    return {
        rawCount: messages.length,
        wireCount: (delta?.wireStart ?? 0) + encoded.length,
        rawLastVisible: rawLast !== undefined && encoded.some((entry) => entry.mid === rawLastId),
        ckFingerprint: ck.fingerprint,
        ckPrefixFingerprintBeforeLast: ck.prefixFingerprintBeforeLast,
        nativeFingerprint: native.fingerprint,
        nativePrefixFingerprintBeforeLast: native.prefixFingerprintBeforeLast,
        rawContentSnapshots: snapshots,
        fingerprint: `${ck.fingerprint}|${native.fingerprint}`,
    };
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
    return `rust pass: decision=${args.decision} reason=${args.reason} served_from=${args.servedFrom} in=${args.inputCount} out=${args.outputCount} applied=${args.applied} row_version=${rowVersion} elapsed=${args.elapsedMs.toFixed(1)} ms module=${args.moduleElapsedMs.toFixed(1)} ms stages=prefix_guard:${timings.prefixGuard.toFixed(1)} ordinal_resolve:${timings.ordinalResolve.toFixed(1)} clone:${timings.clone.toFixed(1)} wire_build:${timings.wireBuild.toFixed(1)} wire_messages:${timings.wireMessages} transport:${timings.transport.toFixed(1)} transport_pages:${timings.transportPages} transport_bytes:${timings.transportBytes} apply:${timings.apply.toFixed(1)} other:${unattributed.toFixed(1)}`;
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
        input_tokens: usage.inputTokens,
        limit,
        current_total_input_tokens: usage.inputTokens,
        context_limit_tokens: limit,
    };
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

function hasNativeResponseContent(response: Record<string, unknown>): boolean {
    if (typeof response.native_messages === "string" || Array.isArray(response.native_messages)) {
        return true;
    }
    const delta = response.native_messages_delta;
    return isRecord(delta) && Array.isArray(delta.messages);
}

/**
 * Build the candidate output array from a module response. The result is a fresh array of shared
 * references: kept prefix entries come from the acknowledged previous output, and every entry the
 * module returned is used as-is because the module owns healing, ordering, and codec fidelity.
 */
export function buildNativeCandidate(
    response: Record<string, unknown>,
    previous: { messages: readonly unknown[]; fingerprint: string } | undefined,
    reserve: (slots: number) => boolean,
): unknown[] {
    const nativeMessages = response.native_messages;
    if (typeof nativeMessages === "string") {
        // Each JSON array entry needs at least one character plus a separator.
        if (!reserve(Math.ceil(nativeMessages.length / 2)))
            throw new CaptureBudgetExceeded("native candidate array");
        let parsed: unknown;
        try {
            parsed = JSON.parse(nativeMessages) as unknown;
        } catch (error) {
            throw new Error(
                `rust transform native_messages string was not valid JSON: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
        if (!Array.isArray(parsed))
            throw new Error("rust transform native_messages string was not an array");
        return parsed;
    }
    if (Array.isArray(nativeMessages)) {
        if (!reserve(nativeMessages.length))
            throw new CaptureBudgetExceeded("native candidate array");
        return Array.from(nativeMessages);
    }
    const delta = response.native_messages_delta;
    if (!isRecord(delta) || !Array.isArray(delta.messages)) {
        throw new Error("rust transform response omitted native_messages");
    }
    const replaceFrom = delta.replace_from;
    if (
        !previous ||
        delta.after !== previous.fingerprint ||
        typeof replaceFrom !== "number" ||
        !Number.isSafeInteger(replaceFrom) ||
        replaceFrom < 0 ||
        replaceFrom > previous.messages.length
    ) {
        throw new Error(
            "rust transform native_messages_delta did not match the acknowledged output",
        );
    }
    if (!reserve(replaceFrom + delta.messages.length)) {
        throw new CaptureBudgetExceeded("native candidate array");
    }
    return previous.messages.slice(0, replaceFrom).concat(delta.messages);
}

function buildTransformBody(args: {
    sessionId: string;
    input: unknown[];
    nativeMessages: unknown[];
    passInputs: Record<string, unknown>;
    usage?: Record<string, number | boolean>;
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
        caveman_enabled: args.passInputs.caveman_enabled === true,
        caveman_min_chars: args.passInputs.caveman_min_chars ?? 500,
        cache_ttl: args.passInputs.cache_ttl,
    };
}

const CANDIDATE_SLOT_BYTES = 8;
/** WIRE_PROJECTION_FACTOR accounts for the CK text, the native text, the paging parse copy, and the page texts. */
const WIRE_PROJECTION_FACTOR = 4;

type PassDeclineReason =
    | "cleared"
    | "superseded"
    | "capture_bytes"
    | "unsupported_source"
    | "host_container"
    | "source_changed"
    | "invalidated"
    | "deleted"
    | "internal_child";

/** A local refusal prevents publication without counting a daemon failure. */
class PassDeclined extends Error {
    constructor(
        sessionId: string,
        readonly reason: PassDeclineReason,
        detail?: string,
    ) {
        super(`rust session ${sessionId} pass declined: ${reason}${detail ? ` (${detail})` : ""}`);
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

    const markFailure = (sessionId: string, state: RustSessionState, error: unknown): void => {
        state.consecutiveFailures += 1;
        state.failureCount += 1;
        sessionLog.warn(sessionId, "rust transform failed; serving the input unchanged:", error);
    };

    const invalidateWireState = (sessionId: string): void => {
        wireCaches.delete(sessionId);
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
        const hostRejection = hostArrayReplacementRejection(target, 0);
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
        try {
            // Source domain is validated synchronously before any message read.
            const prefixGuardStartedAt = performance.now();
            const inspection = inspectReferenceableMessages(target, lease.remainingBytes);
            if (!inspection.ok) {
                throw new PassDeclined(
                    sessionId,
                    "unsupported_source",
                    `${inspection.rejection.reason} at ${inspection.rejection.path}`,
                );
            }
            inputCount = inspection.messageWireBytes.length;
            charge(inspection.estimatedBytes, `capture charge=${inspection.estimatedBytes}`);
            const captured = captureMessages(target, lease);
            // Later reads use the captured members; the live array is only rechecked against them.
            const messages = captured.members as MessageLike[];
            logStage(sessionId, "prefixGuard", prefixGuardStartedAt, timings, "phase=capture");
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
            const previousWireCache = wireCaches.get(sessionId);
            let wireDelta =
                !state.forceFullWire && previousWireCache
                    ? computeWireDelta(previousWireCache, captured.snapshots)
                    : undefined;
            const reserveWire = (from: number, to: number): void => {
                let bytes = 0;
                for (let index = from; index < to; index += 1)
                    bytes += WIRE_PROJECTION_FACTOR * inspection.messageWireBytes[index];
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
            recheckCapture("preflight");
            // Both verdicts freeze from the first user message in the live array before the DB is consulted; a session whose first user row is not yet persisted otherwise reads as provisional and fails closed.
            resolveCtxReduceAvailabilityFromMessages(sessionId, messages);
            const reduceAvailability = resolveCtxReduceAvailability(sessionId);
            // Pass the module one bool combining the frozen map verdict and OpenCode's live permission decision.
            // Synthesis fails closed when host evidence is provisional or missing.
            resolveTodowriteAvailabilityFromMessages(sessionId, messages);
            const todoAvailability = resolveTodowriteAvailability(sessionId);
            const toolPresent = reduceAvailability.frozen && reduceAvailability.callable;
            const todoToolPresent = await resolveCombinedTodowriteVerdict(
                deps,
                sessionId,
                messages,
                todoAvailability,
            );
            recheckCapture("permission");
            if (preflightError) throw preflightError;
            const usage = passUsageSnapshot;
            const contextLimit =
                resolvedContextLimit && resolvedContextLimit > 0
                    ? resolvedContextLimit
                    : usage && usage.percentage > 0
                      ? Math.round(usage.inputTokens / (usage.percentage / 100))
                      : 128_000;
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
            const promptSurfaceGuidance = deps.promptSurfaceRuntime?.resolveGuidance(
                deps.promptSurface,
                modelKey ?? undefined,
            );
            const promptSurface =
                promptSurfaceGuidance ??
                resolvePromptSurface(deps.promptSurface, modelKey ?? undefined);
            const passInputs: Record<string, unknown> = {
                effective_execute_threshold: threshold,
                auto_search_enabled: deps.autoSearch?.enabled ?? true,
                auto_search_score_threshold: deps.autoSearch?.scoreThreshold ?? 0.6,
                auto_search_min_prompt_chars: deps.autoSearch?.minPromptChars ?? 20,
                history_budget_tokens: historyBudgetTokens,
                clear_reasoning_age: deps.clearReasoningAge,
                caveman_enabled: !isSubagent && deps.cavemanTextCompression?.enabled === true,
                caveman_min_chars: deps.cavemanTextCompression?.minChars ?? 500,
                cache_ttl: resolveCacheTtl(deps.cacheTtl, modelKey ?? undefined),
                is_subagent: isSubagent,
                tool_present: toolPresent,
                todo_tool_present: todoToolPresent,
                prompt_surface_preset: promptSurface.preset,
                prompt_surface_model_key: modelKey,
                prompt_surface_config_identity: promptSurfaceConfigIdentity(deps.promptSurface),
                prompt_surface_tool_descriptions: deps.promptSurface?.tool_descriptions ?? {},
                prompt_surface_guidance_override: promptSurfaceGuidance?.primaryOverride,
                protected_tags: deps.protectedTags ?? DEFAULT_PROTECTED_TAGS,
            };
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
            // A memo generation the scan cannot match forces a full re-prime without an anchor.
            const resetStagedMemo = (): void => {
                stagedMemo.memoGeneration = -1;
            };
            let resolved = await resolveOrdinals(
                wireDelta?.rawStart ?? 0,
                provisionalBase,
                "attempt=first",
            );
            if (!resolved.ok) {
                if (wireDelta) reserveWire(0, wireDelta.rawStart);
                wireDelta = undefined;
                resetStagedMemo();
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

            const projectRoot = options.projectRoot ?? directory;
            state.routeRoot = projectRoot;
            deliveries.projectRoot = projectRoot;
            const wireBuildStartedAt = performance.now();
            const encodedInput = encodeOpenCodeMessagesToCk(resolved.annotatedInput);
            timings.wireMessages = messages.length - (wireDelta?.rawStart ?? 0);
            let pendingWireCache: RustWireCache =
                // An empty delta replaces nothing: the terminal message, its wire visibility, and both before-last fingerprints stay the acknowledged ones.
                wireDelta && previousWireCache && wireDelta.rawStart === messages.length
                    ? { ...previousWireCache, nativeOutput: undefined }
                    : buildWireCache({
                          messages,
                          encoded: encodedInput,
                          snapshots: captured.snapshots,
                          delta: wireDelta,
                      });
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
                    recheckCapture(`page:${index}${detail}`);
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
                result = await sendTransformSeries(payload, `${detail} restart=series`);
                if ("restart" in result) {
                    throw new Error(
                        `rust transform page series restart exhausted: reason=${result.restart.reason}`,
                    );
                }
                return result.response;
            };
            let response = await sendTransformSeriesWithSingleRestart(body, "");
            captureResponseTelemetry(response);
            if (isNeedFullSync(response) || !hasNativeResponseContent(response)) {
                if (isNeedFullSync(response)) {
                    // A cleared or superseded session must not receive the flag.
                    assertCurrentPass();
                    state.forceFullWire = true;
                } else {
                    sessionLog.warn(
                        sessionId,
                        "native_delta_fallback_reason=adapter_response_omitted_native_content retry=full",
                    );
                }
                if (wireDelta) {
                    reserveWire(0, wireDelta.rawStart);
                    let retryResolved = await resolveOrdinals(
                        0,
                        stagedMemo.continuationBase,
                        "retry=full",
                    );
                    if (!retryResolved.ok) {
                        resetStagedMemo();
                        retryResolved = await resolveOrdinals(
                            0,
                            undefined,
                            "retry=full fallback=clean_full",
                        );
                    }
                    if (!retryResolved.ok) {
                        throw new Error(`rust ordinal ${retryResolved.reason} during full retry`);
                    }
                    const retryEncodedInput = encodeOpenCodeMessagesToCk(
                        retryResolved.annotatedInput,
                    );
                    timings.wireMessages = messages.length;
                    pendingWireCache = buildWireCache({
                        messages,
                        encoded: retryEncodedInput,
                        snapshots: captured.snapshots,
                    });
                    const retryWireBuildStartedAt = performance.now();
                    body = buildTransformBody({
                        ...transformBodyBase,
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
                }
                response = await sendTransformSeriesWithSingleRestart(body, " retry=full");
                captureResponseTelemetry(response);
                if (isNeedFullSync(response)) {
                    throw new Error("rust module still requires full sync after a full-array send");
                }
                if (!hasNativeResponseContent(response)) {
                    throw new Error("rust module omitted native content after a full-array retry");
                }
            }
            const appliedDeliveryPassIds = new Set(noteDeliveryPassIds(response));
            const applyStartedAt = performance.now();
            try {
                // Candidate construction and every boundary check run before the host array is touched.
                const candidate = buildNativeCandidate(
                    response,
                    previousWireCache?.nativeOutput
                        ? {
                              messages: previousWireCache.nativeOutput,
                              fingerprint: previousWireCache.fingerprint,
                          }
                        : undefined,
                    (slots) => lease.reserve(slots * CANDIDATE_SLOT_BYTES),
                );
                // Kept prefix entries are host-owned since their publication, so the candidate is inspected before any plain read.
                const candidateInspection = inspectReferenceableMessages(candidate);
                if (!candidateInspection.ok) {
                    throw new Error(
                        `rust transform output is not referenceable: ${candidateInspection.rejection.reason} at ${candidateInspection.rejection.path}`,
                    );
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
                const publishRejection = hostArrayReplacementRejection(target, candidate.length);
                if (publishRejection !== null) {
                    throw new PassDeclined(sessionId, "host_container", publishRejection);
                }
                logStage(sessionId, "apply", applyStartedAt, timings);
                const applyReplaceStartedAt = performance.now();
                // Publication and state promotion are synchronous from here to the lease release.
                replaceHostArrayContents(target, candidate);
                pendingWireCache.nativeOutput = candidate;
                state.ordinals = stagedMemo;
                state.initialized = true;
                state.consecutiveFailures = 0;
                state.forceFullWire = false;
                wireCaches.set(sessionId, pendingWireCache);
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
                sessionLog.debug(sessionId, error instanceof Error ? error.message : String(error));
            } else {
                if (decision.toLowerCase() !== "need_full_sync") decision = "error";
                markFailure(sessionId, state, error);
            }
            finishPass(false);
        }
        return deliveries;
    };

    const run = (sessionId: string, output: { messages: unknown[] }): Promise<void> => {
        const admission = captureAdmission.admit(sessionId);
        if ("declined" in admission) {
            sessionLog.debug(
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
            wireCaches.delete(sessionId);
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
    WIRE_PROJECTION_FACTOR,
    buildNativeCandidate,
    buildTransformBody,
    transformGeometryForWire,
    formatRustPassLog,
    isTransformPageAttemptMismatch,
    hasNativeResponseContent,
    createRustModeTransform,
};
