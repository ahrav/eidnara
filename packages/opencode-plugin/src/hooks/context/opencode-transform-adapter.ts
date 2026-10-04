/**
 * The OpenCode adapter over the transform session client. It owns what only OpenCode has: the
 * `messages.transform` output array it captures and publishes into in place (#824 D21), the
 * session directory, and the OpenCode database reads behind the pass inputs (model, mid-turn
 * state, and the first-user tool policy of #824 D23). Every revision 3 protocol step runs in
 * `transform-session-client`.
 */

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
    resolveEidnaraReduceAvailability,
    resolveTodowriteAvailability,
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
import { encodeOpenCodeMessagesToCk } from "./module-wire";
import { findLastAssistantModelFromOpenCodeDb, isMidTurn } from "./read-session-db";
import { isRawCompactionSummaryInfo } from "./read-session-raw";
import {
    knownSessionDirectory,
    resolveSessionDirectory,
    type SessionDirectoryDeps,
} from "./session-directory";
import type { MessageLike } from "./tag-content-primitives";
import {
    copyWindow,
    defaultTransformCaptureAdmission,
    messageId,
    readOwnDataProperty,
    rootArrayRejection,
    type TransformCaptureAdmission,
} from "./transform-capture";
import { publicationRejection, publishInPlace } from "./transform-publication";
import {
    applyTransformRecipe,
    buildTransformRequest,
    createTransformSessionClient,
    formatRustPassLog,
    isTransformPageAttemptMismatch,
    PassDeclined,
    RETAINED_OUTPUT_BUDGET_BYTES,
    RETAINED_OUTPUT_SESSION_CAPACITY,
    RetainedOutputs,
    type RustModeModuleClient,
    type TransformBoundary,
    type TransformHostView,
    type TransformPassSource,
    type TransformSessionState,
} from "./transform-session-client";

export type { RustModeModuleClient, TransformBoundary };

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

function nonEmptyString(value: unknown): string | undefined {
    return typeof value === "string" && value.length > 0 ? value : undefined;
}

function activeAgentFromMessages(messages: readonly MessageLike[]): string | undefined | null {
    let assistantAgent: string | undefined;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        const info = messages[index]?.info as
            | { role?: unknown; agent?: unknown; mode?: unknown }
            | undefined;
        if (info?.role === "user") return nonEmptyString(info.agent);
        if (info?.role === "assistant")
            assistantAgent ??= nonEmptyString(info.agent) ?? nonEmptyString(info.mode);
    }
    return assistantAgent ?? null;
}

async function resolveCombinedTodowriteVerdict(
    deps: RustModeTransformDeps,
    sessionId: string,
    activeAgent: string | undefined | null,
    availability: ToolAvailabilityVerdict,
): Promise<boolean> {
    if (
        activeAgent === null ||
        !availability.frozen ||
        !availability.callable ||
        deps.compactionOff === true
    )
        return false;

    return !(await todowritePermissionDenied(deps.client, sessionId, activeAgent));
}

export interface RustSessionState extends TransformSessionState {
    /** Consecutive passes whose newest user message is synthetic. A real user message resets it. */
    syntheticTurnCount: number;
    lastObservedUserMessageId: string | null;
    /** One cascade log per run of synthetic turns; the reset that clears `syntheticTurnCount` re-arms it. */
    syntheticCascadeLogged: boolean;
}

/** The adapter's own per-session turn observation, beside the client's pass state. */
type SyntheticTurnState = Pick<
    RustSessionState,
    "syntheticTurnCount" | "lastObservedUserMessageId" | "syntheticCascadeLogged"
>;

export interface RustModeTransformOptions {
    moduleClient: RustModeModuleClient;
    projectRoot?: string;
    /** Shared admission owner; tests inject one with smaller limits. */
    captureAdmission?: TransformCaptureAdmission;
    /** Retained-output budget across sessions; tests inject a smaller one. */
    retainedOutputBudgetBytes?: number;
    /** Largest transform body sent unpaged; tests inject the page limit to exercise paging. */
    unpagedTransformMaxBytes?: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object";
}

function messageInfo(value: unknown): Record<string, unknown> {
    if (!isRecord(value)) return {};
    return isRecord(value.info) ? value.info : value;
}

function newestUserMessage(messages: MessageLike[]): MessageLike | undefined {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        if (messageInfo(messages[index]).role === "user") return messages[index];
    }
    return undefined;
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

function observeSyntheticTurn(state: SyntheticTurnState, messages: MessageLike[]): boolean {
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

function assertNativeBoundary(
    output: readonly unknown[],
    sessionId: string,
    boundaryId: string,
): void {
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

/** The pass inputs an OpenCode request carries, as the adapter computes them. */
interface OpenCodePassInputs {
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
}

function buildTransformBody(
    args: OpenCodePassInputs & {
        sessionId: string;
        boundary: TransformBoundary | null;
        baseRevision: string;
        previousOutputRevision?: string;
        input: unknown[];
        nativeMessages: readonly unknown[];
    },
): Record<string, unknown> {
    return buildTransformRequest(
        {
            sessionId: args.sessionId,
            boundary: args.boundary,
            baseRevision: args.baseRevision,
            previousOutputRevision: args.previousOutputRevision,
            serializerProfile: "opencode-aisdk",
            input: args.input,
            nativeMessages: args.nativeMessages,
        },
        openCodePassFields(args),
    );
}

/** The pass inputs an OpenCode transform request carries after its protocol fields. */
function openCodePassFields(args: OpenCodePassInputs): Record<string, unknown> {
    return {
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

/**
 * OpenCode's message array as discovery reads it. Each hop is an own data read, so a planted
 * proxy, revoked proxy, or accessor reads as no id and none of its traps or getters runs.
 */
export function openCodeHostView(target: readonly unknown[]): TransformHostView {
    return {
        get length() {
            return target.length;
        },
        idAt: (index) => messageId(readOwnDataProperty(target, index)),
    };
}

export function createRustModeTransform(
    deps: RustModeTransformDeps,
    options: RustModeTransformOptions,
): {
    run: (sessionId: string, output: { messages: unknown[] }) => Promise<void>;
    clearSession: (sessionId: string) => void;
    getState: (sessionId: string) => Readonly<RustSessionState>;
} {
    const client = createTransformSessionClient({
        moduleClient: options.moduleClient,
        projectRoot: options.projectRoot,
        retainedOutputBudgetBytes: options.retainedOutputBudgetBytes,
        unpagedTransformMaxBytes: options.unpagedTransformMaxBytes,
    });
    const turns = new Map<string, SyntheticTurnState>();
    const captureAdmission = options.captureAdmission ?? defaultTransformCaptureAdmission;
    const turnState = (sessionId: string): SyntheticTurnState => {
        let state = turns.get(sessionId);
        if (!state) {
            state = {
                syntheticTurnCount: 0,
                lastObservedUserMessageId: null,
                syntheticCascadeLogged: false,
            };
            turns.set(sessionId, state);
        }
        return state;
    };

    /** The pass source over OpenCode's `messages.transform` output array. */
    const passSource = (
        sessionId: string,
        output: { messages: unknown[] },
        target: unknown[],
    ): TransformPassSource => {
        let model: { providerID: string; modelID: string } | undefined;
        let preflightError: unknown;
        let resolvedContextLimit: number | undefined;
        let resolvedWindowGeometry: WindowGeometryResult | undefined;
        return {
            serializerProfile: "opencode-aisdk",
            invocationProfile: "opencode-heuristic",
            host: openCodeHostView(target),
            async preflight() {
                // The root's own `then` and the built-in prototypes are refused before any await.
                const rootRejection = rootArrayRejection(target);
                if (rootRejection)
                    throw new PassDeclined(
                        sessionId,
                        "unsupported_source",
                        `${rootRejection.reason} at ${rootRejection.path}`,
                        rootRejection.reason === "prototype_accessor" ? "warn" : "debug",
                    );
                // The directory names the discovery route, reads no source, and records a host-reported `parentID` the subagent classification reads.
                const directory = await resolveSessionDirectory(deps, sessionId);
                if (deps.isSessionDeleted?.(sessionId)) {
                    deps.onSessionDeletedDuringPreflight?.(sessionId);
                    throw new PassDeclined(sessionId, "deleted");
                }
                if (deps.isInternalChildSession?.(sessionId))
                    throw new PassDeclined(sessionId, "internal_child");
                return directory;
            },
            readWindow: (start, end) => copyWindow(target, start, end),
            idOf: messageId,
            liveWindow: (start, end) =>
                readOwnDataProperty(output, "messages") === target &&
                rootArrayRejection(target) === undefined &&
                target.length === end
                    ? copyWindow(target, start, end)
                    : undefined,
            contextLimit(members) {
                // Each attempt resolves afresh; a rediscovery rerun calls this again.
                preflightError = undefined;
                resolvedContextLimit = undefined;
                resolvedWindowGeometry = undefined;
                model = modelFromMessages(members as MessageLike[]);
                if (!model) {
                    try {
                        model = findLastAssistantModelFromOpenCodeDb(sessionId) ?? undefined;
                    } catch (error) {
                        preflightError = error;
                    }
                }
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
                return resolvedContextLimit && resolvedContextLimit > 0
                    ? resolvedContextLimit
                    : undefined;
            },
            async prepare(members, assertCurrent) {
                const messages = members as MessageLike[];
                const reportedContextLimit =
                    resolvedContextLimit && resolvedContextLimit > 0
                        ? resolvedContextLimit
                        : undefined;
                const modelKey = model
                    ? piModelRefToCanonical(resolveModelKey(model.providerID, model.modelID) ?? "")
                    : null;
                const turn = turnState(sessionId);
                const syntheticTurn = observeSyntheticTurn(turn, messages);
                if (syntheticTurn && turn.syntheticTurnCount >= 3 && !turn.syntheticCascadeLogged) {
                    turn.syntheticCascadeLogged = true;
                    sessionLog.warn(
                        sessionId,
                        `rust synthetic-turn cascade: ${turn.syntheticTurnCount} consecutive synthetic user turns with no real user message`,
                    );
                }
                const passUsageSnapshot = loadContextUsage(deps, sessionId);
                // Both verdicts come from the session's earliest persisted user row, never the window's first user; a missing database freezes fail-open, and an unpersisted row or a read error stays provisional and fails closed.
                const reduceAvailability = resolveEidnaraReduceAvailability(sessionId);
                const todoAvailability = resolveTodowriteAvailability(sessionId);
                const toolPresent = reduceAvailability.frozen && reduceAvailability.callable;
                const activeAgent = activeAgentFromMessages(messages);
                // Every message read above is synchronous; the awaits below read nothing from the source, so a recheck precedes encoding.
                const isSubagent = deps.isSubagentSession(sessionId);
                const systemPromptHash = deps.systemPromptHashFor(sessionId);
                const transformGeometry = transformGeometryForWire(resolvedWindowGeometry);
                assertCurrent();
                // Pass the module one bool combining the frozen map verdict and OpenCode's live permission decision.
                // Synthesis fails closed when host evidence is provisional or missing.
                const todoToolPresent = await resolveCombinedTodowriteVerdict(
                    deps,
                    sessionId,
                    activeAgent,
                    todoAvailability,
                );
                assertCurrent();
                if (preflightError) throw preflightError;
                const usage = passUsageSnapshot;
                // The usage sample's percentage was computed against `resolveContextLimit`, which substitutes the 128k default for a model models.dev cannot name, so inverting it recovers that default rather than a host report.
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
                    ...promptSurfaceWireFields(
                        deps.promptSurfaceRuntime,
                        deps.promptSurface,
                        modelKey,
                    ),
                    protected_tags: deps.protectedTags ?? DEFAULT_PROTECTED_TAGS,
                };
                const usageEntry = deps.contextUsageMap.get(sessionId);
                return {
                    // Compaction summaries stay out of the CK window, as the daemon's CK decoder expects; kept messages carry unfiltered window positions (push returns a truthy length).
                    encodeInput() {
                        const positions: number[] = [];
                        const kept = messages.filter(
                            (message, i) =>
                                !isRawCompactionSummaryInfo(message.info) && positions.push(i + 1),
                        );
                        return encodeOpenCodeMessagesToCk(kept, positions);
                    },
                    fields: openCodePassFields({
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
                            usageEntry?.lastResponseTime !== undefined &&
                            usageEntry.lastResponseTime > 0
                                ? usageEntry.lastResponseTime
                                : undefined,
                        requestObservedAtMs,
                    }),
                };
            },
            validateOutput: (values, boundaryId) =>
                assertNativeBoundary(values, sessionId, boundaryId),
            publicationRejection: (slots) => publicationRejection(target, slots),
            publish: (values, window, boundaryIndex) =>
                publishInPlace(target, values, window, boundaryIndex),
        };
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
        const target = readOwnDataProperty(output, "messages") as unknown[];
        const hostRejection = publicationRejection(target, 0);
        if (hostRejection !== null) {
            sessionLog.debug(
                sessionId,
                `rust transform declined before dispatch: host_container ${hostRejection}`,
            );
            lease.release();
            return Promise.resolve();
        }
        return client.run(sessionId, lease, passSource(sessionId, output, target)).then(() => {});
    };

    return {
        run,
        clearSession(sessionId: string): void {
            // Without a `routeRoot`, the fallback is the root `run` would have used, so the daemon's durable state is still addressed.
            const projectRoot =
                client.clear(sessionId) ??
                options.projectRoot ??
                knownSessionDirectory(deps, sessionId);
            turns.delete(sessionId);
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
        getState(sessionId: string): Readonly<RustSessionState> {
            return { ...client.state(sessionId), ...turnState(sessionId) };
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
