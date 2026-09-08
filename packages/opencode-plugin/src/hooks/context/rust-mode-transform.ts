import { createHash } from "node:crypto";

import { DEFAULT_PROTECTED_TAGS } from "../../features/context/defaults";
import type { PluginContext } from "../../plugin/types";
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
import { withTimeout } from "../../shared/with-timeout";
import {
    cachedToolPermissionDenied,
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
    buildPagedModuleTransformPayloads,
    encodeOpenCodeMessagesToCk,
    type ModuleMethod,
    type ModuleOrdinalMemo,
    resolveOrdinalsForModule,
} from "./module-wire";
import { findLastAssistantModelFromOpenCodeDb, isMidTurn } from "./read-session-db";
import type { RawMessageOrdinalAnchor } from "./read-session-raw";
import type { MessageLike } from "./tag-content-primitives";
import { logTransformTiming } from "./transform-stage-logger";

export interface RustModeTransformDeps {
    contextUsageMap: Map<string, ContextUsageEntry>;
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
    client?: PluginContext["client"];
    directory?: string;
    sessionDirectoryBySession?: Map<string, string>;
    isSubagentSession: (sessionId: string) => boolean;
    systemPromptHashFor: (sessionId: string) => string;
}

function activeAgentFromMessages(messages: readonly MessageLike[]): string | undefined {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        const info = messages[index]?.info as { role?: unknown; agent?: unknown } | undefined;
        if (info?.role !== "user") continue;
        return typeof info.agent === "string" && info.agent.length > 0 ? info.agent : undefined;
    }
    return undefined;
}

/** Limits OpenCode SDK reads so a slow host cannot block the transform indefinitely. */
const HOST_READ_TIMEOUT_MS = 2_000;

async function resolveCombinedTodowriteVerdict(
    deps: RustModeTransformDeps,
    sessionId: string,
    messages: readonly MessageLike[],
    availability: ToolAvailabilityVerdict,
): Promise<boolean> {
    if (!availability.frozen || !availability.callable || deps.compactionOff === true) return false;

    let permissionDenied = cachedToolPermissionDenied(sessionId, "todowrite") ?? false;
    if (deps.client) {
        try {
            permissionDenied = await withTimeout(
                todowritePermissionDenied(
                    deps.client,
                    sessionId,
                    activeAgentFromMessages(messages),
                ),
                HOST_READ_TIMEOUT_MS,
                "todowrite permission read timed out",
            );
        } catch (error) {
            // A failed or slow SDK read leaves the last in-memory verdict unchanged until a later read obtains authoritative data.
            sessionLog(
                sessionId,
                "todowrite permission read failed; retaining the last successful verdict:",
                error,
            );
        }
    }
    return !permissionDenied;
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
}

type ContentSnapshotField = string | number | boolean | symbol;

/** Wire caches hold a session's content snapshots and its last native output, so the LRU bound is sized to the sessions one OpenCode process keeps active. An evicted session sends its next pass as a full array. */
const WIRE_CACHE_SESSION_CAPACITY = 64;

const SNAPSHOT_ARRAY = Symbol("array");
const SNAPSHOT_OBJECT = Symbol("object");
const SNAPSHOT_KEY = Symbol("key");
const SNAPSHOT_STRING = Symbol("string");
const SNAPSHOT_NUMBER = Symbol("number");
const SNAPSHOT_BOOLEAN = Symbol("boolean");
const SNAPSHOT_NULL = Symbol("null");
const SNAPSHOT_UNDEFINED = Symbol("undefined");

interface MessageContentSnapshot {
    signature: string;
    fields: ContentSnapshotField[];
}

interface RustWireCache {
    rawCount: number;
    wireCount: number;
    rawLastId: string | null;
    rawLastSignature: string | null;
    rawLastVisible: boolean;
    /** Each pass re-verifies reused messages so in-place edits cannot reuse a stale prefix. */
    rawContentSnapshots: MessageContentSnapshot[];
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
    /** `invalidateWireState` increments this value. A pass commits its cache only when the value
     * matches the one it read alongside the previous cache, so an invalidation that lands during
     * the daemon call survives that pass's completion. */
    wireInvalidations: number;
    moduleGeneration: number;
    idOrdinalMemoGeneration: number;
    idOrdinalMemo: Map<string, number>;
    ordinalMemoAnchor: RawMessageOrdinalAnchor | null;
    ordinalMemoStoredCount: number | null;
    ordinalMemoCanonicalCount: number;
    /** The module returns a durable prior-lineage tail after descent.
     * The loop continues after `base` without regenerating `index + 1` ordinals. */
    ordinalContinuationBase: number | null;
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
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object";
}

/**
 * OpenCode retains the original messages array when it serializes a transform result.
 * OpenCode requires in-place mutation of its original `messages` array for the module response to reach the wire.
 */
function replaceMessagesInPlace(output: { messages: unknown[] }, next: unknown[]): unknown[] {
    const target = output.messages;
    if (target !== next) target.splice(0, target.length, ...next);
    return target;
}

function messageInfo(value: unknown): Record<string, unknown> {
    if (!isRecord(value)) return {};
    return isRecord(value.info) ? value.info : value;
}

function messageIdOf(message: MessageLike): string | null {
    const id = messageInfo(message).id;
    return typeof id === "string" && id.length > 0 ? id : null;
}

const FNV1A_32_OFFSET = 0x811c9dc5;
const FNV1A_32_PRIME = 0x01000193;

function updateFnv1a32(hash: number, value: string): number {
    let next = hash;
    for (let index = 0; index < value.length; index += 1) {
        next ^= value.charCodeAt(index);
        next = Math.imul(next, FNV1A_32_PRIME) >>> 0;
    }
    return next;
}

interface MessageContentFieldVisitor {
    field(value: ContentSnapshotField): boolean;
    beginObject(): number | undefined;
    endObject(token: number, entryCount: number): boolean;
}

function isSnapshotObjectChild(value: unknown): boolean {
    return value !== undefined && typeof value !== "function" && typeof value !== "symbol";
}

function visitMessageContentFields(value: unknown, visitor: MessageContentFieldVisitor): boolean {
    if (value === null) return visitor.field(SNAPSHOT_NULL);
    if (typeof value === "string") {
        return visitor.field(SNAPSHOT_STRING) && visitor.field(value);
    }
    if (typeof value === "number") {
        return visitor.field(SNAPSHOT_NUMBER) && visitor.field(value);
    }
    if (typeof value === "boolean") {
        return visitor.field(SNAPSHOT_BOOLEAN) && visitor.field(value);
    }
    if (value === undefined || typeof value === "function" || typeof value === "symbol") {
        return visitor.field(SNAPSHOT_UNDEFINED);
    }
    if (Array.isArray(value)) {
        if (!visitor.field(SNAPSHOT_ARRAY) || !visitor.field(value.length)) return false;
        for (const item of value) {
            if (!visitMessageContentFields(item, visitor)) return false;
        }
        return true;
    }
    if (typeof value === "object") {
        if (!visitor.field(SNAPSHOT_OBJECT)) return false;
        const objectToken = visitor.beginObject();
        if (objectToken === undefined) return false;
        let entryCount = 0;
        for (const key in value) {
            if (!Object.hasOwn(value, key)) continue;
            const child = (value as Record<string, unknown>)[key];
            if (!isSnapshotObjectChild(child)) continue;
            entryCount += 1;
            if (
                !visitor.field(SNAPSHOT_KEY) ||
                !visitor.field(key) ||
                !visitMessageContentFields(child, visitor)
            ) {
                return false;
            }
        }
        return visitor.endObject(objectToken, entryCount);
    }
    return visitor.field(SNAPSHOT_UNDEFINED);
}

function messageContentFields(message: MessageLike): ContentSnapshotField[] {
    const fields: ContentSnapshotField[] = [];
    const complete = visitMessageContentFields(message, {
        field(value) {
            fields.push(value);
            return true;
        },
        beginObject() {
            const countIndex = fields.length;
            fields.push(0);
            return countIndex;
        },
        endObject(countIndex, entryCount) {
            fields[countIndex] = entryCount;
            return true;
        },
    });
    if (!complete) throw new Error("message content snapshot traversal stopped unexpectedly");
    return fields;
}

function signatureForFields(fields: readonly ContentSnapshotField[]): string {
    let hash = FNV1A_32_OFFSET;
    for (const field of fields) {
        const value = typeof field === "symbol" ? (field.description ?? "") : String(field);
        hash = updateFnv1a32(hash, `${typeof field}:${value.length}:`);
        hash = updateFnv1a32(hash, value);
        hash = updateFnv1a32(hash, "\0");
    }
    return hash.toString(16).padStart(8, "0");
}

function messageContentSnapshot(message: MessageLike): MessageContentSnapshot {
    const fields = messageContentFields(message);
    return { signature: signatureForFields(fields), fields };
}

function contentSnapshotsFor(messages: readonly MessageLike[]): MessageContentSnapshot[] {
    return messages.map(messageContentSnapshot);
}

function messageMatchesContentSnapshot(
    message: MessageLike,
    snapshot: MessageContentSnapshot,
): boolean {
    let fieldIndex = 0;
    const matched = visitMessageContentFields(message, {
        field(value) {
            if (!Object.is(value, snapshot.fields[fieldIndex])) return false;
            fieldIndex += 1;
            return true;
        },
        beginObject() {
            const expectedCount = snapshot.fields[fieldIndex];
            if (typeof expectedCount !== "number") return undefined;
            fieldIndex += 1;
            return expectedCount;
        },
        endObject(expectedCount, entryCount) {
            return expectedCount === entryCount;
        },
    });
    return matched && fieldIndex === snapshot.fields.length;
}

function prefixContentSnapshotsMatch(
    messages: readonly MessageLike[],
    cache: RustWireCache,
    prefixLength: number,
): boolean {
    if (prefixLength > cache.rawContentSnapshots.length) return false;
    for (let index = 0; index < prefixLength; index += 1) {
        if (!messageMatchesContentSnapshot(messages[index], cache.rawContentSnapshots[index])) {
            return false;
        }
    }
    return true;
}

function messageCacheSignature(message: MessageLike): string {
    const parts = Array.isArray(message.parts) ? message.parts : [];
    const serializedParts = JSON.stringify(parts) ?? "null";
    const serializedMessage = JSON.stringify(message) ?? "null";
    return `${messageIdOf(message) ?? ""}:${parts.length}:${Buffer.byteLength(serializedParts)}:${createHash("sha256").update(serializedMessage).digest("hex")}`;
}

function advanceWireFingerprint(previous: string, encoded: unknown): string {
    return createHash("sha256")
        .update(previous)
        .update("\\0")
        .update(JSON.stringify(encoded) ?? "null")
        .digest("hex");
}

function buildWireFingerprint(encoded: unknown[]): {
    fingerprint: string;
    prefixFingerprintBeforeLast: string;
} {
    let fingerprint = "rust-wire-v1";
    let prefixFingerprintBeforeLast = fingerprint;
    for (let index = 0; index < encoded.length; index += 1) {
        if (index === encoded.length - 1) prefixFingerprintBeforeLast = fingerprint;
        fingerprint = advanceWireFingerprint(fingerprint, encoded[index]);
    }
    return { fingerprint, prefixFingerprintBeforeLast };
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

function isRustGenerationRecord(value: unknown): value is Record<string, number> {
    return (
        isRecord(value) &&
        Object.entries(value).every(
            ([projectId, generation]) =>
                /^\d+$/.test(projectId) &&
                typeof generation === "number" &&
                Number.isSafeInteger(generation) &&
                generation >= 0,
        )
    );
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
            moduleGeneration: 0,
            idOrdinalMemoGeneration: 0,
            idOrdinalMemo: new Map(),
            ordinalMemoAnchor: null,
            ordinalMemoStoredCount: null,
            ordinalMemoCanonicalCount: 0,
            ordinalContinuationBase: null,
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

function knownSessionDirectory(deps: RustModeTransformDeps, sessionId: string): string {
    return deps.sessionDirectoryBySession?.get(sessionId) ?? deps.directory ?? process.cwd();
}

async function getSessionDirectory(
    deps: RustModeTransformDeps,
    sessionId: string,
): Promise<string> {
    const cached = deps.sessionDirectoryBySession?.get(sessionId);
    if (cached) return cached;
    if (!deps.client?.session?.get) return knownSessionDirectory(deps, sessionId);
    try {
        const response = await withTimeout(
            deps.client.session.get({ path: { id: sessionId } }),
            HOST_READ_TIMEOUT_MS,
            "session directory read timed out",
        );
        const directory = (response as { data?: { directory?: unknown } } | null)?.data?.directory;
        if (typeof directory === "string" && directory.length > 0) {
            deps.sessionDirectoryBySession?.set(sessionId, directory);
            return directory;
        }
    } catch {
        // Module routing falls back to the launch directory without failing.
    }
    return knownSessionDirectory(deps, sessionId);
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

export function applyNativeMessagesVerbatim(
    output: { messages: unknown[] },
    response: Record<string, unknown>,
    previous?: { messages: readonly unknown[]; fingerprint: string },
): unknown[] {
    const nativeMessages = response.native_messages;
    if (typeof nativeMessages === "string") {
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
        return replaceMessagesInPlace(output, parsed);
    }
    if (Array.isArray(nativeMessages)) {
        // The module owns healing, ordering, and codec fidelity; do not clone, normalize, or inspect the returned native message array.
        return replaceMessagesInPlace(output, nativeMessages);
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
    return replaceMessagesInPlace(output, [
        ...previous.messages.slice(0, replaceFrom),
        ...delta.messages,
    ]);
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

export function createRustModeTransform(
    deps: RustModeTransformDeps,
    options: RustModeTransformOptions,
): {
    run: (
        sessionId: string,
        messages: MessageLike[],
        output: { messages: unknown[] },
    ) => Promise<void>;
    clearSession: (sessionId: string) => void;
    invalidateWireState: (sessionId: string) => void;
    getState: (sessionId: string) => Readonly<RustSessionState>;
} {
    const states = new Map<string, RustSessionState>();
    const wireCaches = new BoundedSessionMap<RustWireCache>(WIRE_CACHE_SESSION_CAPACITY);

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

    const callModule = (args: Parameters<RustModeModuleClient["call"]>[0]): Promise<unknown> =>
        options.moduleClient.call(args);

    const markFailure = (sessionId: string, state: RustSessionState, error: unknown): void => {
        state.consecutiveFailures += 1;
        state.failureCount += 1;
        sessionLog(sessionId, "rust transform failed; serving the input unchanged:", error);
    };

    const resetOrdinalMemo = (state: RustSessionState): void => {
        state.idOrdinalMemo.clear();
        state.ordinalMemoAnchor = null;
        state.ordinalMemoStoredCount = null;
        state.ordinalMemoCanonicalCount = 0;
    };

    // The single projection from session state to the resolver's memo bundle;
    // evaluated at call time so a preceding resetOrdinalMemo is observed.
    const ordinalMemoOf = (state: RustSessionState): ModuleOrdinalMemo => ({
        generation: state.moduleGeneration,
        memoGeneration: state.idOrdinalMemoGeneration,
        entries: state.idOrdinalMemo,
        anchor: state.ordinalMemoAnchor,
        storedCount: state.ordinalMemoStoredCount,
        canonicalCount: state.ordinalMemoCanonicalCount,
        continuationBase: state.ordinalContinuationBase ?? 0,
    });

    const invalidateWireState = (sessionId: string): void => {
        wireCaches.delete(sessionId);
        const state = states.get(sessionId);
        if (!state) return;
        resetOrdinalMemo(state);
        state.forceFullWire = true;
        state.wireInvalidations += 1;
    };

    const run = async (
        sessionId: string,
        messages: MessageLike[],
        output: { messages: unknown[] },
    ): Promise<void> => {
        const passStartedAt = performance.now();
        const state = ensureState(states, sessionId);
        const timings = emptyRustPassTimings();
        state.passCount += 1;
        const syntheticTurn = observeSyntheticTurn(state, messages);
        if (syntheticTurn && state.syntheticTurnCount >= 3 && !state.syntheticCascadeLogged) {
            state.syntheticCascadeLogged = true;
            sessionLog(
                sessionId,
                `rust synthetic-turn cascade: ${state.syntheticTurnCount} consecutive synthetic user turns with no real user message`,
            );
        }
        const inputCount = messages.length;
        let decision = "error";
        let materializeReason = "none";
        let servedFrom = "none";
        let moduleElapsedMs = 0;
        let rowVersion = 0;
        let appliedAt: number | undefined;
        const passUsageSnapshot = loadContextUsage(deps, sessionId);
        const isSubagent = deps.isSubagentSession(sessionId);
        const systemPromptHash = deps.systemPromptHashFor(sessionId);
        let preflightError: unknown;
        let model = modelFromMessages(messages);
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
                resolvedContextLimit = resolveTrustedContextLimit(model.providerID, model.modelID);
                resolvedWindowGeometry = resolveContextWindowGeometry(
                    model.providerID,
                    model.modelID,
                );
            } catch (error) {
                preflightError ??= error;
            }
        }
        const transformGeometry = transformGeometryForWire(resolvedWindowGeometry);
        const finishPass = (applied: boolean): void => {
            const elapsedAt = applied && appliedAt !== undefined ? appliedAt : performance.now();
            const elapsedMs = Math.max(0, elapsedAt - passStartedAt);
            sessionLog(
                sessionId,
                formatRustPassLog({
                    decision,
                    reason: materializeReason,
                    servedFrom,
                    inputCount,
                    outputCount: output.messages.length,
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
                sessionLog(
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
                if (detail) sessionLog(sessionId, `rust module stages (slow pass): ${detail}`);
            }
        };
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
        try {
            if (preflightError) throw preflightError;
            const directory = await getSessionDirectory(deps, sessionId);
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
            const previousWireCache = wireCaches.get(sessionId);
            // Every await after this read lets `invalidateWireState` or `clearSession` run; the commit below compares against this value.
            const wireInvalidationsAtRead = state.wireInvalidations;
            let wireDelta:
                | {
                      rawStart: number;
                      wireStart: number;
                      after: string;
                      ckAfter: string;
                      nativeAfter: string;
                  }
                | undefined;
            if (
                !state.forceFullWire &&
                previousWireCache &&
                messages.length >= previousWireCache.rawCount
            ) {
                const appending = messages.length > previousWireCache.rawCount;
                const formerTerminal = messages[previousWireCache.rawCount - 1];
                // Use delta transport only when the reusable prefix byte-identically matches OpenCode's copy.
                // The prefix guard stops before the former terminal; the signature check below covers it.
                const prefixGuardStartedAt = performance.now();
                const prefixIntact = prefixContentSnapshotsMatch(
                    messages,
                    previousWireCache,
                    Math.max(0, previousWireCache.rawCount - 1),
                );
                logStage(sessionId, "prefixGuard", prefixGuardStartedAt, timings);
                // When appending, an invisible former terminal may have been edited in place, so only a visible former terminal skips the signature check.
                const lastChanged =
                    formerTerminal !== undefined &&
                    !(appending && previousWireCache.rawLastVisible) &&
                    messageCacheSignature(formerTerminal) !== previousWireCache.rawLastSignature;
                const replaceExistingTail =
                    lastChanged || (appending && previousWireCache.rawLastVisible);
                const rawStart = replaceExistingTail
                    ? Math.max(0, previousWireCache.rawCount - 1)
                    : previousWireCache.rawCount;
                const replaceExistingWireTail =
                    previousWireCache.rawLastVisible && (lastChanged || appending);
                const wireStart = replaceExistingWireTail
                    ? Math.max(0, previousWireCache.wireCount - 1)
                    : previousWireCache.wireCount;
                const ckAfter =
                    wireStart === previousWireCache.wireCount - 1
                        ? previousWireCache.ckPrefixFingerprintBeforeLast
                        : wireStart === previousWireCache.wireCount
                          ? previousWireCache.ckFingerprint
                          : undefined;
                const nativeAfter =
                    rawStart === previousWireCache.rawCount - 1
                        ? previousWireCache.nativePrefixFingerprintBeforeLast
                        : rawStart === previousWireCache.rawCount
                          ? previousWireCache.nativeFingerprint
                          : undefined;
                if (prefixIntact && ckAfter !== undefined && nativeAfter !== undefined) {
                    wireDelta = {
                        rawStart,
                        wireStart,
                        ckAfter,
                        nativeAfter,
                        after: previousWireCache.fingerprint,
                    };
                }
            }
            const cloneStartedAt = performance.now();
            const ordinalMessages = wireDelta ? messages.slice(wireDelta.rawStart) : messages;
            logStage(
                sessionId,
                "clone",
                cloneStartedAt,
                timings,
                wireDelta ? "mode=projection-tail" : "mode=projection-full",
            );
            const provisionalBase = wireDelta
                ? (() => {
                      for (let index = wireDelta.rawStart - 1; index >= 0; index -= 1) {
                          const priorId = messageIdOf(messages[index]);
                          if (!priorId) continue;
                          const prior = state.idOrdinalMemo.get(priorId);
                          if (prior !== undefined)
                              return Math.max(prior, state.ordinalContinuationBase ?? 0);
                      }
                      return Math.max(
                          state.ordinalMemoCanonicalCount,
                          state.ordinalContinuationBase ?? 0,
                      );
                  })()
                : (state.ordinalContinuationBase ?? undefined);
            const ordinalStartedAt = performance.now();
            let resolved = await resolveOrdinalsForModule({
                sessionId,
                messages: ordinalMessages,
                memo: ordinalMemoOf(state),
                provisionalBase,
            });
            logStage(sessionId, "ordinalResolve", ordinalStartedAt, timings);
            if (!resolved.ok) {
                wireDelta = undefined;
                resetOrdinalMemo(state);
                const fullOrdinalStartedAt = performance.now();
                resolved = await resolveOrdinalsForModule({
                    sessionId,
                    messages,
                    memo: ordinalMemoOf(state),
                    provisionalBase: state.ordinalContinuationBase ?? undefined,
                });
                logStage(
                    sessionId,
                    "ordinalResolve",
                    fullOrdinalStartedAt,
                    timings,
                    "fallback=clean_full",
                );
            }
            if (!resolved.ok) {
                throw new Error(
                    `rust ordinal ${resolved.reason}: messageId=${resolved.messageId ?? "unknown"} ` +
                        `index=${resolved.messageIndex ?? "unknown"} role=${resolved.messageRole ?? "unknown"}`,
                );
            }
            state.idOrdinalMemoGeneration = resolved.memoGeneration;
            state.ordinalMemoAnchor = resolved.memoAnchor;
            state.ordinalMemoStoredCount = resolved.memoStoredCount;
            state.ordinalMemoCanonicalCount = resolved.memoCanonicalCount;

            const projectRoot = options.projectRoot ?? directory;
            state.routeRoot = projectRoot;
            const wireBuildStartedAt = performance.now();
            const encodedInput = encodeOpenCodeMessagesToCk(resolved.annotatedInput);
            timings.wireMessages = wireDelta
                ? messages.length - wireDelta.rawStart
                : messages.length;
            let pendingWireCache: RustWireCache = (() => {
                const rawLast = messages.at(-1);
                if (!wireDelta || !previousWireCache) {
                    const ckFingerprint = buildWireFingerprint(encodedInput);
                    const nativeFingerprint = buildWireFingerprint(messages);
                    return {
                        rawCount: messages.length,
                        wireCount: encodedInput.length,
                        rawLastId: rawLast ? messageIdOf(rawLast) : null,
                        rawLastSignature: rawLast ? messageCacheSignature(rawLast) : null,
                        rawLastVisible:
                            rawLast !== undefined &&
                            encodedInput.some((entry) => entry.mid === messageIdOf(rawLast)),
                        ckFingerprint: ckFingerprint.fingerprint,
                        ckPrefixFingerprintBeforeLast: ckFingerprint.prefixFingerprintBeforeLast,
                        nativeFingerprint: nativeFingerprint.fingerprint,
                        nativePrefixFingerprintBeforeLast:
                            nativeFingerprint.prefixFingerprintBeforeLast,
                        rawContentSnapshots: contentSnapshotsFor(messages),
                        fingerprint: `${ckFingerprint.fingerprint}|${nativeFingerprint.fingerprint}`,
                    };
                }
                const nativeMessages = messages.slice(wireDelta.rawStart);
                if (nativeMessages.length === 0) {
                    // An empty delta replaces nothing: the terminal message, its wire visibility, and both before-last fingerprints stay the acknowledged ones. Recomputing them from an empty tail would record the terminal as invisible and chain the before-last fingerprints off the full array.
                    return { ...previousWireCache, nativeOutput: undefined };
                }
                let ckFingerprint = wireDelta.ckAfter;
                let ckPrefixFingerprintBeforeLast = ckFingerprint;
                for (let index = 0; index < encodedInput.length; index += 1) {
                    if (index === encodedInput.length - 1)
                        ckPrefixFingerprintBeforeLast = ckFingerprint;
                    ckFingerprint = advanceWireFingerprint(ckFingerprint, encodedInput[index]);
                }
                let nativeFingerprint = wireDelta.nativeAfter;
                let nativePrefixFingerprintBeforeLast = nativeFingerprint;
                for (let index = 0; index < nativeMessages.length; index += 1) {
                    if (index === nativeMessages.length - 1)
                        nativePrefixFingerprintBeforeLast = nativeFingerprint;
                    nativeFingerprint = advanceWireFingerprint(
                        nativeFingerprint,
                        nativeMessages[index],
                    );
                }
                const rawLastVisible =
                    rawLast !== undefined &&
                    encodedInput.some((entry) => entry.mid === messageIdOf(rawLast));
                return {
                    rawCount: messages.length,
                    wireCount: wireDelta.wireStart + encodedInput.length,
                    rawLastId: rawLast ? messageIdOf(rawLast) : null,
                    rawLastSignature: rawLast ? messageCacheSignature(rawLast) : null,
                    rawLastVisible,
                    ckFingerprint,
                    ckPrefixFingerprintBeforeLast,
                    nativeFingerprint,
                    nativePrefixFingerprintBeforeLast,
                    rawContentSnapshots: [
                        ...previousWireCache.rawContentSnapshots.slice(0, wireDelta.rawStart),
                        ...contentSnapshotsFor(messages.slice(wireDelta.rawStart)),
                    ],
                    fingerprint: `${ckFingerprint}|${nativeFingerprint}`,
                };
            })();
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
            const sendTransformSeries = async (
                payload: Record<string, unknown>,
                detail = "",
            ): Promise<TransformSeriesResult> => {
                const pages = buildPagedModuleTransformPayloads(payload);
                const paged = pages.some(
                    (entry) => typeof entry.page.transform_page_id === "string",
                );
                let response: Record<string, unknown> | undefined;
                for (const [index, { page, bytes }] of pages.entries()) {
                    const transportStartedAt = performance.now();
                    let moduleResponse: unknown;
                    try {
                        moduleResponse = await callModule({
                            sessionId,
                            projectRoot,
                            method: "transform",
                            body: page,
                            generationSensitive: paged && index > 0,
                        });
                    } catch (error) {
                        if (paged && isTransformPageAttemptMismatch(error)) {
                            return {
                                restart: {
                                    reason: "attempt_mismatch",
                                    pages: pages.length,
                                    atPage: index,
                                },
                            };
                        }
                        throw error;
                    }
                    if (paged && isModuleTransportGenerationChangedResult(moduleResponse)) {
                        return {
                            restart: { reason: "reconnect", pages: pages.length, atPage: index },
                        };
                    }
                    if (paged && isTransformPageAttemptMismatch(moduleResponse)) {
                        return {
                            restart: {
                                reason: "attempt_mismatch",
                                pages: pages.length,
                                atPage: index,
                            },
                        };
                    }
                    response = responseValue(moduleResponse);
                    timings.transportBytes += bytes;
                    timings.transportPages += 1;
                    logStage(
                        sessionId,
                        "transport",
                        transportStartedAt,
                        timings,
                        `page=${index + 1}/${pages.length}${detail}`,
                    );
                }
                if (!response) throw new Error("rust module returned no transform response");
                return { response };
            };
            let transformSeriesRestarted = false;
            const sendTransformSeriesWithSingleRestart = async (
                payload: Record<string, unknown>,
                detail = "",
            ): Promise<Record<string, unknown>> => {
                let result = await sendTransformSeries(payload, detail);
                if (!("restart" in result)) return result.response;
                if (transformSeriesRestarted) {
                    throw new Error(
                        `rust transform page series restart exhausted: reason=${result.restart.reason}`,
                    );
                }
                transformSeriesRestarted = true;
                sessionLog(
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
            let response = await sendTransformSeriesWithSingleRestart(body);
            captureResponseTelemetry(response);
            const needFullSync = isNeedFullSync(response);
            const nativeContentOmitted = !hasNativeResponseContent(response);
            if (needFullSync || nativeContentOmitted) {
                if (!needFullSync) {
                    sessionLog(
                        sessionId,
                        "native_delta_fallback_reason=adapter_response_omitted_native_content retry=full",
                    );
                }
                state.forceFullWire = true;
                if (wireDelta) {
                    const retryOrdinalStartedAt = performance.now();
                    let retryResolved = await resolveOrdinalsForModule({
                        sessionId,
                        messages,
                        memo: ordinalMemoOf(state),
                        provisionalBase: state.ordinalContinuationBase ?? undefined,
                    });
                    logStage(
                        sessionId,
                        "ordinalResolve",
                        retryOrdinalStartedAt,
                        timings,
                        "retry=full",
                    );
                    if (!retryResolved.ok) {
                        resetOrdinalMemo(state);
                        retryResolved = await resolveOrdinalsForModule({
                            sessionId,
                            messages,
                            memo: ordinalMemoOf(state),
                        });
                    }
                    if (!retryResolved.ok) {
                        throw new Error(`rust ordinal ${retryResolved.reason} during full retry`);
                    }
                    state.idOrdinalMemoGeneration = retryResolved.memoGeneration;
                    state.ordinalMemoAnchor = retryResolved.memoAnchor;
                    state.ordinalMemoStoredCount = retryResolved.memoStoredCount;
                    state.ordinalMemoCanonicalCount = retryResolved.memoCanonicalCount;
                    const retryEncodedInput = encodeOpenCodeMessagesToCk(
                        retryResolved.annotatedInput,
                    );
                    timings.wireMessages = messages.length;
                    const retryCkFingerprint = buildWireFingerprint(retryEncodedInput);
                    const retryNativeFingerprint = buildWireFingerprint(messages);
                    const retryRawLast = messages.at(-1);
                    pendingWireCache = {
                        rawCount: messages.length,
                        wireCount: retryEncodedInput.length,
                        rawLastId: retryRawLast ? messageIdOf(retryRawLast) : null,
                        rawLastSignature: retryRawLast ? messageCacheSignature(retryRawLast) : null,
                        rawLastVisible:
                            retryRawLast !== undefined &&
                            retryEncodedInput.some(
                                (entry) => entry.mid === messageIdOf(retryRawLast),
                            ),
                        ckFingerprint: retryCkFingerprint.fingerprint,
                        ckPrefixFingerprintBeforeLast:
                            retryCkFingerprint.prefixFingerprintBeforeLast,
                        nativeFingerprint: retryNativeFingerprint.fingerprint,
                        nativePrefixFingerprintBeforeLast:
                            retryNativeFingerprint.prefixFingerprintBeforeLast,
                        rawContentSnapshots: contentSnapshotsFor(messages),
                        fingerprint: `${retryCkFingerprint.fingerprint}|${retryNativeFingerprint.fingerprint}`,
                    };
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
            const deliveryPassIds = noteDeliveryPassIds(response);
            const sendNoteDeliveryDisposition = async (
                method: "transform.ack" | "transform.nack",
            ) => {
                for (const transformPassId of deliveryPassIds) {
                    await callModule({
                        sessionId,
                        projectRoot,
                        method,
                        body: {
                            method,
                            v: 1,
                            session_id: sessionId,
                            transform_pass_id: transformPassId,
                        },
                    });
                }
            };
            let appliedMessages: unknown[];
            const applyStartedAt = performance.now();
            try {
                appliedMessages = applyNativeMessagesVerbatim(
                    { messages: [] },
                    response,
                    previousWireCache?.nativeOutput
                        ? {
                              messages: previousWireCache.nativeOutput,
                              fingerprint: previousWireCache.fingerprint,
                          }
                        : undefined,
                );
                pendingWireCache.nativeOutput = appliedMessages;
                const boundaryId = response.boundary_id;
                if (typeof boundaryId === "string" && boundaryId.length > 0) {
                    assertNativeBoundary(appliedMessages, sessionId, boundaryId);
                }
                logStage(sessionId, "apply", applyStartedAt, timings);
                const applyReplaceStartedAt = performance.now();
                replaceMessagesInPlace(output, appliedMessages);
                logStage(sessionId, "apply", applyReplaceStartedAt, timings);
            } catch (error) {
                logStage(sessionId, "apply", applyStartedAt, timings, "failed=true");
                try {
                    await sendNoteDeliveryDisposition("transform.nack");
                } catch (nackError) {
                    sessionLog(sessionId, "rust note delivery nack failed (ignored):", nackError);
                }
                throw error;
            }
            if (deliveryPassIds.length > 0) {
                try {
                    await sendNoteDeliveryDisposition("transform.ack");
                } catch (ackError) {
                    sessionLog(sessionId, "rust note delivery ack failed (will retry):", ackError);
                }
            }
            const ordinalContinuationBase = response.ordinal_continuation_base;
            if (
                typeof ordinalContinuationBase === "number" &&
                Number.isSafeInteger(ordinalContinuationBase) &&
                ordinalContinuationBase > 0
            ) {
                if (state.ordinalContinuationBase === null) {
                    for (const [messageId, ordinal] of state.idOrdinalMemo) {
                        state.idOrdinalMemo.set(messageId, ordinal + ordinalContinuationBase);
                    }
                    state.ordinalMemoCanonicalCount += ordinalContinuationBase;
                }
                state.ordinalContinuationBase = ordinalContinuationBase;
            }
            state.initialized = true;
            state.consecutiveFailures = 0;
            if (
                states.get(sessionId) === state &&
                state.wireInvalidations === wireInvalidationsAtRead
            ) {
                state.forceFullWire = false;
                wireCaches.set(sessionId, pendingWireCache);
            } else {
                // The session was cleared or its wire state invalidated while this pass awaited the daemon. The applied output stands; the cache built from the pre-invalidation array does not, and `forceFullWire` keeps the invalidator's value.
                sessionLog(
                    sessionId,
                    "rust wire state changed during the pass; discarding this pass's wire cache",
                );
            }
            appliedAt = performance.now();
            finishPass(true);
        } catch (error) {
            servedFrom = "raw";
            if (decision.toLowerCase() !== "need_full_sync") decision = "error";
            materializeReason = "none";
            markFailure(sessionId, state, error);
            replaceMessagesInPlace(output, messages);
            finishPass(false);
            return;
        }
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
            if (options.moduleClient.deleteSession) {
                void options.moduleClient
                    .deleteSession(sessionId, projectRoot)
                    .catch((error) => {
                        sessionLog(sessionId, "rust module session deletion failed:", error);
                    })
                    .finally(() => options.moduleClient.closeSession?.(sessionId));
            } else {
                options.moduleClient.closeSession?.(sessionId);
            }
        },
        invalidateWireState,
        getState(sessionId: string): Readonly<RustSessionState> {
            return {
                ...ensureState(states, sessionId),
                idOrdinalMemo: new Map(ensureState(states, sessionId).idOrdinalMemo),
            };
        },
    };
}

export const __rustModeTransformTest = {
    WIRE_CACHE_SESSION_CAPACITY,
    applyNativeMessagesVerbatim,
    contentSnapshotsFor,
    snapshotTags: {
        array: SNAPSHOT_ARRAY,
        object: SNAPSHOT_OBJECT,
        key: SNAPSHOT_KEY,
        string: SNAPSHOT_STRING,
        number: SNAPSHOT_NUMBER,
        boolean: SNAPSHOT_BOOLEAN,
        null: SNAPSHOT_NULL,
        undefined: SNAPSHOT_UNDEFINED,
    },
    messageContentSnapshot,
    messageMatchesContentSnapshot,
    buildTransformBody,
    transformGeometryForWire,
    formatRustPassLog,
    isRustGenerationRecord,
    isTransformPageAttemptMismatch,
    hasNativeResponseContent,
    createRustModeTransform,
};
