import { isCompactionEnabled, isContextResearcherRunnable } from "../../config/agent-disable";
import type { ContextResearcherConfig } from "../../config/schema/eidnara";
import {
    clearHookInitFailure,
    recordHookInitFailure,
} from "../../features/context/fail-closed-block";
import { resolveProjectIdentityForSession } from "../../features/context/project-identity";
import {
    type RustToolBackends,
    RustToolSessionDeletedError,
} from "../../plugin/rust-tool-backends";
import type { PluginContext } from "../../plugin/types";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { log, sessionLog } from "../../shared/logger";
import {
    CAPTURE_MAX_AGE_MS,
    createMemoryCaptureCheckpoint,
    createMemoryCaptureDrain,
    type MemoryCaptureDrain,
    type MemoryCaptureScope,
    openCodeCaptureMessages,
    openCodeLastFinalMessageId,
    openCodeMessagesSince,
} from "../../shared/memory-capture";
import { normalizeSDKResponse } from "../../shared/normalize-sdk-response";
import type { PromptSurfaceConfig } from "../../shared/prompt-surface";
import type { PromptSurfaceRuntime } from "../../shared/prompt-surface-runtime";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "../../shared/with-timeout";
import { createEidnaraCommandHandler } from "./command-handler";
import { invalidateToolPermissionDenied } from "./eidnara-reduce-availability";
import { type ContextUsageEntry, createEventHandler } from "./event-handler";
import { createGuidanceFetcher } from "./guidance-fetch";
import {
    createChatMessageHook,
    createCommandExecuteBeforeHook,
    createEventHook,
    createToolExecuteAfterHook,
    getLiveNotificationParams,
} from "./hook-handlers";
import { closeKernelSession, kernelClientResolver } from "./kernel-transport";
import {
    addBoundedSession,
    type LiveSessionState,
    MAX_LIVE_USAGE_SESSIONS,
} from "./live-session-state";
import { openCodeMemoryCaptureExecutor } from "./memory-capture-native";
import { findLastAssistantModelFromOpenCodeDb } from "./read-session-db";
import { createRustModeTransform, type RustModeModuleClient } from "./rust-mode-transform";
import { sendIgnoredMessage } from "./send-session-notification";
import { resolveSessionDirectory, type SessionDirectoryResolver } from "./session-directory";
import { createSystemPromptHashHandler } from "./system-prompt-hash";
import type { MessageLike } from "./tag-content-primitives";
import { createTextCompleteHandler } from "./text-complete";
import { readOwnDataProperty } from "./transform-capture";

export type { CommandExecuteInput, CommandExecuteOutput } from "./command-handler";

export interface EidnaraDeps {
    client: PluginContext["client"];
    directory: string;
    onSessionCacheInvalidated?: (sessionId: string) => void;
    liveSessionState?: LiveSessionState;
    config: {
        protected_tags: number;
        /** User-level setting that lets a session started exactly in the canonical home directory use it as the project. */
        allow_home_project?: boolean;
        language?: string;
        toast_duration_ms?: number;
        clear_reasoning_age?: number;
        execute_threshold_percentage?: number | { default: number; [modelKey: string]: number };
        execute_threshold_tokens?: { default?: number; [modelKey: string]: number | undefined };
        cache_ttl: string | Record<string, string>;
        prompt_surface?: PromptSurfaceConfig;
        history_budget_percentage?: number;
        memory?: {
            enabled: boolean;
            auto_promote?: boolean;
            auto_capture?: boolean;
            injection_budget_tokens: number;
            auto_search?: {
                enabled: boolean;
                score_threshold: number;
                min_prompt_chars: number;
            };
        };
        context_researcher?: ContextResearcherConfig;
        /** Optional because Zod `.default()` supplies it in loaded configs. */
        system_prompt_injection?: { enabled: boolean; skip_signatures: string[] };
        terse_text_compression?: {
            enabled: boolean;
            min_chars: number;
        };
        host?: { connection_file: string };
        /** Compaction-off mode gate. Resolved ONCE here at the
         *  session-hook construction boundary via isCompactionEnabled; the
         *  resolved boolean is threaded to the transform phases. */
        compaction?: { enabled?: boolean };
    };
    /** Registration owns `promptSurfaceRuntime` and shares it with the tool registry. */
    promptSurfaceRuntime?: PromptSurfaceRuntime;
    /** The daemon client the caller owns and disconnects; the hook never dials a transport of its own. */
    rustModeModuleClient: RustModeModuleClient;
}

/**
 * The transform receives no session id of its own; every message carries it in `info.sessionID`.
 * The read goes through own-data descriptors because the source guard has not run yet.
 */
function resolveSessionId(messages: readonly MessageLike[]): string | undefined {
    const sessionId = readOwnDataProperty(
        readOwnDataProperty(readOwnDataProperty(messages, "0"), "info"),
        "sessionID",
    );
    return typeof sessionId === "string" && sessionId.length > 0 ? sessionId : undefined;
}

export function createEidnaraHook(deps: EidnaraDeps) {
    const contextUsageMap =
        deps.liveSessionState?.contextUsageBySession ??
        new BoundedSessionMap<ContextUsageEntry>(MAX_LIVE_USAGE_SESSIONS);

    clearHookInitFailure();
    const projectPath = resolveProjectIdentityForSession(
        deps.directory,
        deps.config.allow_home_project,
    );
    if (!projectPath) {
        log("[eidnara] not binding a project identity for this directory");
        recordHookInitFailure({ type: "no_project" });
        return null;
    }

    const historyRefreshSessions =
        deps.liveSessionState?.historyRefreshSessions ?? new Set<string>();
    const deferredHistoryRefreshSessions =
        deps.liveSessionState?.deferredHistoryRefreshSessions ?? new Set<string>();
    const systemPromptRefreshSessions =
        deps.liveSessionState?.systemPromptRefreshSessions ?? new Set<string>();
    const pendingMaterializationSessions =
        deps.liveSessionState?.pendingMaterializationSessions ?? new Set<string>();
    const deferredMaterializationSessions =
        deps.liveSessionState?.deferredMaterializationSessions ?? new Set<string>();
    const lastHeuristicsTurnId = new Map<string, string>();
    const variantBySession =
        deps.liveSessionState?.variantBySession ?? new Map<string, string | undefined>();
    const liveModelBySession =
        deps.liveSessionState?.liveModelBySession ??
        new Map<string, { providerID: string; modelID: string }>();
    const agentBySession = deps.liveSessionState?.agentBySession ?? new Map<string, string>();
    const sessionDirectoryBySession =
        deps.liveSessionState?.sessionDirectoryBySession ?? new Map<string, string>();
    const sessionMetadataReadStateBySession =
        deps.liveSessionState?.sessionMetadataReadStateBySession ?? new Map();
    const internalChildSessions = deps.liveSessionState?.internalChildSessions ?? new Set<string>();
    const subagentSessions = deps.liveSessionState?.subagentSessions ?? new Set<string>();
    // One resolver serves the transform, the commands, the todo snapshots, and the ContextResearcher child, so every daemon call for a session shares one route root.
    const sessionDirectoryDeps = {
        client: deps.client,
        directory: deps.directory,
        sessionDirectoryBySession,
        sessionMetadataReadStateBySession,
        subagentSessions,
        internalChildSessions,
    };
    const sessionDirectoryFor: SessionDirectoryResolver = (sessionId, fallbackDirectory) =>
        resolveSessionDirectory(sessionDirectoryDeps, sessionId, fallbackDirectory);
    // Sessions deleted in this process; a detached write that resolves after the deletion must not recreate daemon state for them.
    const deletedSessions = new Set<string>();
    const clearDeletedSessionRoutingState = (sessionId: string): void => {
        sessionDirectoryBySession.delete(sessionId);
        sessionMetadataReadStateBySession.delete(sessionId);
        internalChildSessions.delete(sessionId);
        subagentSessions.delete(sessionId);
    };
    // isSubagentSession waits for sessionDirectoryFor because the directory read classifies restored child sessions.
    const isSubagentSession = async (sessionId: string): Promise<boolean> => {
        await sessionDirectoryFor(sessionId);
        if (deletedSessions.has(sessionId)) {
            clearDeletedSessionRoutingState(sessionId);
            return false;
        }
        return subagentSessions.has(sessionId);
    };
    const projectRootForLiveSession: SessionDirectoryResolver = async (
        sessionId,
        fallbackDirectory,
    ): Promise<string> => {
        if (deletedSessions.has(sessionId)) throw new RustToolSessionDeletedError();
        const projectRoot = await sessionDirectoryFor(sessionId, fallbackDirectory);
        if (deletedSessions.has(sessionId)) {
            clearDeletedSessionRoutingState(sessionId);
            throw new RustToolSessionDeletedError();
        }
        return projectRoot;
    };
    const projectRootForCommand = async (sessionId: string): Promise<string> => {
        const projectRoot = await sessionDirectoryFor(sessionId);
        if (deletedSessions.has(sessionId)) clearDeletedSessionRoutingState(sessionId);
        return projectRoot;
    };

    /**
     * `resolveLiveModel` prefers entries in `liveModelBySession` populated by chat and event hooks.
     * It falls back to the last assistant model in OpenCode's SQLite DB when `/eidnara-status` runs
     * before any hook populates the map, and caches that result for later calls.
     */
    const resolveLiveModel = (
        sessionId: string,
    ): { providerID: string; modelID: string } | undefined => {
        const cached = liveModelBySession.get(sessionId);
        if (cached) return cached;
        const recovered = findLastAssistantModelFromOpenCodeDb(sessionId);
        if (recovered) {
            liveModelBySession.set(sessionId, recovered);
            return recovered;
        }
        return undefined;
    };

    // Compaction-off mode is resolved once here and threaded to every phase as a boolean.
    const compactionOff = !isCompactionEnabled(deps.config);
    const context_researcherConfig = isContextResearcherRunnable(deps.config)
        ? deps.config.context_researcher
        : undefined;
    const moduleClient = deps.rustModeModuleClient;
    const captureCheckpoint = createMemoryCaptureCheckpoint(moduleClient);
    const executeCapture = openCodeMemoryCaptureExecutor(deps.client);
    const notifyCaptureIncomplete = () =>
        deps.client.tui.showToast({
            body: {
                title: "Eidnara memory capture",
                message:
                    "Capture incomplete. Some facts are not confirmed saved. See Eidnara logs.",
                variant: "warning",
            },
        });
    /** One warning per project outage: the toast repeats only after that project's drain ends
     * with no work left. A checkpoint alone, a `pending` drain (retry backoff, another claimant),
     * or another project's recovery cannot re-arm it, or a persistent model outage would warn on
     * every eligible retry. */
    const captureWarned = new Set<string>();
    const warnCaptureIncomplete = (projectRoot: string): void => {
        if (captureWarned.has(projectRoot)) return;
        captureWarned.add(projectRoot);
        // `.then` turns a synchronous throw from a disposed client into a rejection this swallows.
        void withTimeout(
            Promise.resolve().then(notifyCaptureIncomplete),
            HOST_SDK_READ_TIMEOUT_MS,
            "capture notification timed out",
        ).catch(() => undefined);
    };
    // Model batches run detached from the idle checkpoint that schedules them, so the idle event
    // returns before any extraction work. One drain per completed turn sees the user's message
    // and the answer together.
    const memoryCaptureDrain = createMemoryCaptureDrain(moduleClient, executeCapture, {
        onSettled: (scope, result) => {
            if (result !== "pending") captureWarned.delete(scope.projectRoot);
        },
        onFailed: (scope, error) => {
            sessionLog.warn(scope.sessionId, "memory capture drain failed:", error);
            warnCaptureIncomplete(scope.projectRoot);
        },
    });
    const liveModelKey = (sessionId: string): string | undefined => {
        const model = resolveLiveModel(sessionId);
        return model ? `${model.providerID}/${model.modelID}` : undefined;
    };
    const pendingUserCaptures = new BoundedSessionMap<Promise<void>>(MAX_LIVE_USAGE_SESSIONS);
    /** Per session, the last final message an idle checkpoint offered; later checkpoints read past it. */
    const captureWatermark = new BoundedSessionMap<string>(MAX_LIVE_USAGE_SESSIONS);
    /** Messages read back per later checkpoint before falling back to the whole transcript. */
    const CAPTURE_TAIL_MESSAGES = 32;
    const captureDisabled = (): boolean =>
        deps.config.memory?.enabled === false ||
        deps.config.memory?.auto_promote === false ||
        deps.config.memory?.auto_capture === false;
    /** Capture excludes deleted and child sessions. Callers re-check after `sessionDirectoryFor`
     * because that read can add `sessionId` to the child-session sets. */
    const excludedFromCapture = (sessionId: string): boolean =>
        deletedSessions.has(sessionId) ||
        subagentSessions.has(sessionId) ||
        internalChildSessions.has(sessionId);
    const checkpointUser = (sessionId: string, output: unknown): void => {
        if (captureDisabled()) return;
        try {
            const messages = [
                ...openCodeCaptureMessages([
                    {
                        info: readOwnDataProperty(output, "message"),
                        parts: readOwnDataProperty(output, "parts"),
                    },
                ]),
            ];
            if (messages.length === 0) return;
            const pending = (async () => {
                const projectRoot = await sessionDirectoryFor(sessionId);
                if (excludedFromCapture(sessionId)) return;
                const model = liveModelBySession.get(sessionId);
                await captureCheckpoint({
                    sessionId,
                    projectRoot,
                    model: model ? `${model.providerID}/${model.modelID}` : undefined,
                    messages,
                });
            })();
            pendingUserCaptures.set(sessionId, pending);
            void pending
                .finally(() => {
                    if (pendingUserCaptures.get(sessionId) === pending)
                        pendingUserCaptures.delete(sessionId);
                })
                .catch((error) =>
                    log(
                        `memory capture user checkpoint pending: ${error instanceof Error ? error.message : "unknown error"}`,
                    ),
                );
        } catch (error) {
            log(
                `memory capture user snapshot failed: ${error instanceof Error ? error.message : "unknown error"}`,
            );
        }
    };
    const checkpointMemory = async (sessionId: string): Promise<void> => {
        if (captureDisabled() || excludedFromCapture(sessionId)) return;
        let scope: MemoryCaptureScope | undefined;
        try {
            const model = liveModelKey(sessionId);
            // The user's message is acknowledged first, so the transcript read below does not resend it.
            await pendingUserCaptures.get(sessionId)?.catch(() => undefined);
            const projectRoot = await sessionDirectoryFor(sessionId);
            // That read can classify this session as a child; a child never checkpoints or drains.
            if (excludedFromCapture(sessionId)) return;
            // From here the user checkpoint may have queued a source, so the drain runs whatever
            // happens to the transcript read or this checkpoint.
            scope = { sessionId, projectRoot, model };
            const readTranscript = async (limit?: number): Promise<unknown[]> =>
                normalizeSDKResponse(
                    await withTimeout(
                        Promise.resolve(
                            deps.client.session.messages({
                                path: { id: sessionId },
                                query: { directory: projectRoot, limit },
                            } as never),
                        ),
                        HOST_SDK_READ_TIMEOUT_MS,
                        "memory capture transcript read timed out",
                    ),
                    [] as unknown[],
                    { preferResponseOnMissingData: true },
                );
            // The first checkpoint offers the whole recent transcript; later ones read only the
            // tail past the last final message this process already offered.
            const since = captureWatermark.get(sessionId);
            const sourceMessages =
                since === undefined
                    ? await readTranscript()
                    : (openCodeMessagesSince(
                          await readTranscript(CAPTURE_TAIL_MESSAGES),
                          since,
                          CAPTURE_TAIL_MESSAGES,
                      ) ?? (await readTranscript()));
            const accepted = await captureCheckpoint({
                ...scope,
                messages: openCodeCaptureMessages(sourceMessages, {
                    notBefore: Date.now() - CAPTURE_MAX_AGE_MS,
                }),
            });
            // A disabled daemon wrote nothing; those messages stay ahead of the watermark.
            if (accepted !== "accepted") return;
            const final = openCodeLastFinalMessageId(sourceMessages);
            if (final !== undefined) captureWatermark.set(sessionId, final);
        } catch (error) {
            log(
                `memory capture checkpoint pending: ${error instanceof Error ? error.message : "unknown error"}`,
            );
            warnCaptureIncomplete(scope?.projectRoot ?? deps.directory);
        } finally {
            // Draining is what frees a full queue, so a refused checkpoint must not skip it.
            if (scope) memoryCaptureDrain.schedule(scope);
        }
    };

    const rustToolBackends: RustToolBackends = {
        reduce: async ({ sessionId, drop, commandId }) => {
            const projectRoot = await projectRootForLiveSession(sessionId);
            return moduleClient.call({
                sessionId,
                projectRoot,
                method: "agent_drops.append",
                body: {
                    method: "agent_drops.append",
                    v: 1,
                    session_id: sessionId,
                    drop,
                    command_id: commandId,
                },
            });
        },
        note: async ({
            commandId,
            sessionId,
            action,
            content,
            surfaceCondition,
            filter,
            limit,
            offset,
            noteId,
        }) => {
            const projectRoot = await projectRootForLiveSession(sessionId);
            const memoryProject = resolveProjectIdentityForSession(
                projectRoot,
                deps.config.allow_home_project,
            );
            if (memoryProject === undefined) {
                throw new Error("Could not resolve project identity for eidnara_note.");
            }
            return moduleClient.call({
                sessionId,
                projectRoot,
                method: "eidnara_note",
                body: {
                    name: "eidnara_note",
                    arguments: {
                        ...(commandId ? { command_id: commandId } : {}),
                        action,
                        content,
                        memory_project: memoryProject,
                        surface_condition: surfaceCondition,
                        filter,
                        limit,
                        offset,
                        note_id: noteId,
                    },
                },
            });
        },
    };

    // Guidance comes from the daemon, which is already on the prompt path and serves the tags
    // and blocks the guidance explains.
    const fetchGuidance = createGuidanceFetcher({
        moduleClient,
        projectRootForSession: projectRootForLiveSession,
        promptSurfaceRuntime: deps.promptSurfaceRuntime,
        promptSurface: deps.config.prompt_surface,
        language: deps.config.language,
    });

    const systemPromptHash = createSystemPromptHashHandler({
        promptSurface: deps.config.prompt_surface,
        resolveModel: resolveLiveModel,
        fetchGuidance,
        isSubagentSession: (sessionId) => subagentSessions.has(sessionId),
        historyRefreshSessions,
        systemPromptRefreshSessions,
        pendingMaterializationSessions,
        lastHeuristicsTurnId,
        injectionEnabled: deps.config.system_prompt_injection?.enabled ?? true,
        injectionSkipSignatures: deps.config.system_prompt_injection?.skip_signatures ?? [
            "<!-- eidnara: skip -->",
        ],
        internalChildSessions,
    });

    const rustTransform = createRustModeTransform(
        {
            contextUsageMap,
            protectedTags: deps.config.protected_tags,
            clearReasoningAge: deps.config.clear_reasoning_age ?? 50,
            executeThresholdPercentage: deps.config.execute_threshold_percentage,
            executeThresholdTokens: deps.config.execute_threshold_tokens,
            historyBudgetPercentage: deps.config.history_budget_percentage,
            promptSurface: deps.config.prompt_surface,
            promptSurfaceRuntime: deps.promptSurfaceRuntime,
            terse_text_compressionTextCompression: compactionOff
                ? undefined
                : deps.config.terse_text_compression?.enabled === true
                  ? {
                        enabled: true,
                        minChars: deps.config.terse_text_compression.min_chars ?? 500,
                    }
                  : undefined,
            autoSearch: {
                enabled: deps.config.memory?.auto_search?.enabled ?? true,
                scoreThreshold: deps.config.memory?.auto_search?.score_threshold ?? 0.6,
                minPromptChars: deps.config.memory?.auto_search?.min_prompt_chars ?? 20,
            },
            cacheTtl: deps.config.cache_ttl,
            compactionOff,
            ...sessionDirectoryDeps,
            isSubagentSession: (sessionId) => subagentSessions.has(sessionId),
            systemPromptHashFor: (sessionId) =>
                systemPromptHash.promptStateFor(sessionId)?.systemPromptHash ?? "",
            // The transform owns the directory read, so admission precedes it; these gates run once the read classifies the session.
            isSessionDeleted: (sessionId) => deletedSessions.has(sessionId),
            onSessionDeletedDuringPreflight: clearDeletedSessionRoutingState,
            isInternalChildSession: (sessionId) => internalChildSessions.has(sessionId),
        },
        // No `projectRoot` option: the transform routes each session by its own resolved directory.
        { moduleClient },
    );

    const messagesTransform = async (
        _input: unknown,
        output: { messages: unknown[] },
    ): Promise<void> => {
        const messages = readOwnDataProperty(output, "messages") as MessageLike[];
        const sessionId = resolveSessionId(messages);
        if (!sessionId) return;
        if (deletedSessions.has(sessionId)) return;
        await rustTransform.run(sessionId, output);
    };

    const eventHandler = createEventHandler({
        contextUsageMap,
        client: deps.client,
        internalChildSessions,
        subagentSessions,
        onSessionCacheInvalidated: (sessionId: string) => {
            deps.onSessionCacheInvalidated?.(sessionId);
        },
        onRustWireInvalidated: (sessionId: string) => {
            rustTransform.invalidateWireState(sessionId);
        },
        onNewestResponseRemoved: (sessionId: string, model) => {
            if (model) liveModelBySession.set(sessionId, model);
            else liveModelBySession.delete(sessionId);
        },
        // Deletion prunes per-session state so entries do not outlive the session.
        onSessionDeleted: (sessionId: string, directory?: string) => {
            addBoundedSession(deletedSessions, sessionId);
            if (directory && !sessionDirectoryBySession.has(sessionId)) {
                sessionDirectoryBySession.set(sessionId, directory);
            }
            rustTransform.clearSession(sessionId);
            // Memory reads from hooks, tools, and sidebar polls hold kernel routes on the shared transport; host route capacity is finite.
            closeKernelSession(deps.config, sessionId);
            systemPromptHash.clearSession(sessionId);
            fetchGuidance.clearSession(sessionId);
            lastHeuristicsTurnId.delete(sessionId);
            variantBySession.delete(sessionId);
            liveModelBySession.delete(sessionId);
            agentBySession.delete(sessionId);
            clearDeletedSessionRoutingState(sessionId);
        },
    });

    const commandHandler = createEidnaraCommandHandler({
        moduleClient,
        kernelClient: kernelClientResolver(deps.config),
        compactionOff,
        resolveProjectRoot: projectRootForCommand,
        isSessionDeleted: (sessionId) => deletedSessions.has(sessionId),
        isSubagentSession,
        // The DB fallback gives /eidnara-status the model-specific threshold before the first hook after a restart.
        getLiveModelKey: (sessionId) => {
            const model = resolveLiveModel(sessionId);
            return model ? `${model.providerID}/${model.modelID}` : undefined;
        },
        // /eidnara-flush signals history rebuild, system-prompt adjuncts, and forced materialization.
        onFlush: (sessionId) => {
            invalidateToolPermissionDenied(sessionId);
            historyRefreshSessions.add(sessionId);
            systemPromptRefreshSessions.add(sessionId);
            pendingMaterializationSessions.add(sessionId);
        },
        sendNotification: async (sessionId, text, params) => {
            const notificationParams = {
                ...getLiveNotificationParams(
                    sessionId,
                    liveModelBySession,
                    variantBySession,
                    agentBySession,
                    deps.config.toast_duration_ms,
                ),
                ...params,
            };
            await sendIgnoredMessage(
                deps.client,
                sessionId,
                text,
                notificationParams,
                params.forcePersist === true,
            );
        },
        context_researcher: context_researcherConfig
            ? {
                  config: context_researcherConfig,
                  projectPath,
                  resolveSessionDirectory: sessionDirectoryFor,
                  client: deps.client,
                  language: deps.config.language,
              }
            : undefined,
    });

    const eventHook = createEventHook({
        eventHandler,
        checkpointMemory,
        contextUsageMap,
        liveModelBySession,
        variantBySession,
        agentBySession,
        sessionDirectoryBySession,
        historyRefreshSessions,
        deferredHistoryRefreshSessions,
        systemPromptRefreshSessions,
        pendingMaterializationSessions,
        deferredMaterializationSessions,
        lastHeuristicsTurnId,
        client: deps.client,
        protectedTags: deps.config.protected_tags,
    });

    const hooks = {
        "experimental.chat.messages.transform": messagesTransform,
        "experimental.chat.system.transform": systemPromptHash.handler,
        "experimental.text.complete": createTextCompleteHandler(),
        "chat.message": createChatMessageHook({
            checkpointUser,
            liveModelBySession,
            variantBySession,
            agentBySession,
        }),
        event: eventHook,
        "command.execute.before": createCommandExecuteBeforeHook(commandHandler),
        "tool.execute.after": createToolExecuteAfterHook({
            subagentSessions,
            client: deps.client,
            todoStateSet: async ({ sessionId, stateJson, ownerMessageId }) => {
                if (deletedSessions.has(sessionId) || subagentSessions.has(sessionId)) {
                    return undefined;
                }
                const projectRoot = await sessionDirectoryFor(sessionId);
                // Session deletion and child classification may complete during the directory read.
                if (deletedSessions.has(sessionId)) {
                    clearDeletedSessionRoutingState(sessionId);
                    return undefined;
                }
                if (subagentSessions.has(sessionId)) {
                    return undefined;
                }
                return moduleClient.call({
                    sessionId,
                    projectRoot,
                    method: "todo_state.set",
                    body: {
                        method: "todo_state.set",
                        v: 1,
                        session_id: sessionId,
                        state_json: stateJson,
                        owner_message_id: ownerMessageId,
                    },
                });
            },
        }),
    };
    const hooksWithBackends = hooks as typeof hooks & {
        rustToolBackends: RustToolBackends;
        resolveSessionDirectory: typeof sessionDirectoryFor;
        memoryCaptureDrain: MemoryCaptureDrain;
    };
    Object.defineProperty(hooksWithBackends, "rustToolBackends", {
        value: rustToolBackends,
        enumerable: false,
    });
    Object.defineProperty(hooksWithBackends, "resolveSessionDirectory", {
        value: projectRootForLiveSession,
        enumerable: false,
    });
    Object.defineProperty(hooksWithBackends, "memoryCaptureDrain", {
        value: memoryCaptureDrain,
        enumerable: false,
    });
    return hooksWithBackends;
}

export async function createEidnaraHookAsync(
    deps: EidnaraDeps,
): Promise<ReturnType<typeof createEidnaraHook>> {
    return createEidnaraHook(deps);
}
