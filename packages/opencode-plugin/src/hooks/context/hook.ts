import { isCompactionEnabled, isSidekickRunnable } from "../../config/agent-disable";
import type { SidekickConfig } from "../../config/schema/eidnara";
import type { ResolvedTransformMode } from "../../config/transform-mode";
import {
    clearHookInitFailure,
    recordHookInitFailure,
} from "../../features/context/fail-closed-block";
import { resolveProjectIdentityForSession } from "../../features/context/project-identity";
import type { RustToolBackends } from "../../plugin/rust-tool-backends";
import type { PluginContext } from "../../plugin/types";
import { log } from "../../shared/logger";
import type { PromptSurfaceConfig } from "../../shared/prompt-surface";
import type { PromptSurfaceRuntime } from "../../shared/prompt-surface-runtime";
import { createEidnaraCommandHandler } from "./command-handler";
import { clearToolPermissionDenied } from "./ctx-reduce-availability";
import { type ContextUsageEntry, createEventHandler } from "./event-handler";
import {
    createChatMessageHook,
    createCommandExecuteBeforeHook,
    createEventHook,
    createToolExecuteAfterHook,
    getLiveNotificationParams,
} from "./hook-handlers";
import { addBoundedSession, type LiveSessionState } from "./live-session-state";
import { HostModuleTransport } from "./module-transport";
import { findLastAssistantModelFromOpenCodeDb } from "./read-session-db";
import { createRustModeTransform, type RustModeModuleClient } from "./rust-mode-transform";
import { sendIgnoredMessage } from "./send-session-notification";
import { resolveSessionDirectory } from "./session-directory";
import { createSystemPromptHashHandler } from "./system-prompt-hash";
import type { MessageLike } from "./tag-content-primitives";
import { createTextCompleteHandler } from "./text-complete";

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
            injection_budget_tokens: number;
            auto_search?: {
                enabled: boolean;
                score_threshold: number;
                min_prompt_chars: number;
            };
        };
        sidekick?: SidekickConfig;
        /** Optional because Zod `.default()` supplies it in loaded configs. */
        system_prompt_injection?: { enabled: boolean; skip_signatures: string[] };
        caveman_text_compression?: {
            enabled: boolean;
            min_chars: number;
        };
        transform_mode?: ResolvedTransformMode;
        subc?: { connection_file: string };
        /** Compaction-off mode gate. Resolved ONCE here at the
         *  session-hook construction boundary via isCompactionEnabled; the
         *  resolved boolean is threaded to the transform phases. */
        compaction?: { enabled?: boolean };
    };
    /** Registration owns `promptSurfaceRuntime` and shares it with the tool registry. */
    promptSurfaceRuntime?: PromptSurfaceRuntime;
    /** `rustModeModuleClient` lets tests replace the daemon adapter; production creates the subc client. */
    rustModeModuleClient?: RustModeModuleClient;
}

/** The transform receives no session id of its own; every message carries it in `info.sessionID`. */
function resolveSessionId(messages: readonly MessageLike[]): string | undefined {
    const info = messages[0]?.info as { sessionID?: unknown } | undefined;
    return typeof info?.sessionID === "string" && info.sessionID.length > 0
        ? info.sessionID
        : undefined;
}

export function createEidnaraHook(deps: EidnaraDeps) {
    const contextUsageMap = new Map<string, ContextUsageEntry>();

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
    // One resolver serves the transform, the commands, the todo snapshots, and the Sidekick child, so every daemon call for a session shares one route root.
    const sessionDirectoryDeps = {
        client: deps.client,
        directory: deps.directory,
        sessionDirectoryBySession,
        sessionMetadataReadStateBySession,
        subagentSessions,
        internalChildSessions,
    };
    const sessionDirectoryFor = (sessionId: string): Promise<string> =>
        resolveSessionDirectory(sessionDirectoryDeps, sessionId);
    // The directory read is also what classifies a restored child, so a gate that reads the sets waits for it first.
    const isSubagentSession = async (sessionId: string): Promise<boolean> => {
        await sessionDirectoryFor(sessionId);
        return subagentSessions.has(sessionId);
    };
    // Sessions deleted in this process; a detached write that resolves after the deletion must not recreate daemon state for them.
    const deletedSessions = new Set<string>();

    /**
     * `resolveLiveModel` prefers entries in `liveModelBySession` populated by chat and event hooks.
     * It falls back to the last assistant model in OpenCode's SQLite DB when `/ctx-status` runs
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
    const sidekickConfig = isSidekickRunnable(deps.config) ? deps.config.sidekick : undefined;
    const rustMode = deps.config.transform_mode === "rust";

    const moduleClient: RustModeModuleClient =
        deps.rustModeModuleClient ??
        (() => {
            const transport = new HostModuleTransport(deps.config.subc?.connection_file);
            const client: RustModeModuleClient = {
                call: (args) => transport.call(args),
                deleteSession: (sessionId, projectRoot) =>
                    transport.deleteSession(sessionId, projectRoot),
                closeSession: (sessionId) => transport.closeSession(sessionId),
            };
            return client;
        })();

    const rustToolBackends: RustToolBackends | undefined = rustMode
        ? {
              reduce: ({ sessionId, projectRoot, drop, commandId }) =>
                  moduleClient.call({
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
                  }),
              note: ({
                  commandId,
                  sessionId,
                  projectRoot,
                  memoryProject,
                  action,
                  content,
                  surfaceCondition,
                  compiledProvider,
                  compiledConfig,
                  compiledAt,
                  compileStatus,
                  filter,
                  limit,
                  offset,
                  noteId,
              }) =>
                  moduleClient.call({
                      sessionId,
                      projectRoot,
                      method: "ctx_note",
                      body: {
                          name: "ctx_note",
                          arguments: {
                              ...(commandId ? { command_id: commandId } : {}),
                              action,
                              content,
                              memory_project: memoryProject,
                              surface_condition: surfaceCondition,
                              ...(compileStatus
                                  ? {
                                        compiled_provider: compiledProvider,
                                        compiled_config: compiledConfig,
                                        compiled_at: compiledAt,
                                        compile_status: compileStatus,
                                    }
                                  : {}),
                              filter,
                              limit,
                              offset,
                              note_id: noteId,
                          },
                      },
                  }),
              // No `noteEvaluationAvailable`: conditioned notes require a live `note.evaluation.register` heartbeat.
          }
        : undefined;

    const systemPromptHash = createSystemPromptHashHandler({
        promptSurface: deps.config.prompt_surface,
        resolveModel: resolveLiveModel,
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
            cavemanTextCompression: compactionOff
                ? undefined
                : deps.config.caveman_text_compression?.enabled === true
                  ? {
                        enabled: true,
                        minChars: deps.config.caveman_text_compression.min_chars ?? 500,
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
        },
        // No `projectRoot` option: the transform routes each session by its own resolved directory.
        { moduleClient },
    );

    // `ts` mode leaves messages untouched; the plugin-level adapter passes them through.
    const messagesTransform = rustMode
        ? async (_input: unknown, output: { messages: unknown[] }): Promise<void> => {
              const messages = output.messages as MessageLike[];
              const sessionId = resolveSessionId(messages);
              if (!sessionId) return;
              if (deletedSessions.has(sessionId)) return;
              // Hidden `eidnara-` children run Eidnara's own prompts and receive no project context; the directory read classifies a child restored after a restart.
              await sessionDirectoryFor(sessionId);
              if (deletedSessions.has(sessionId)) return;
              if (internalChildSessions.has(sessionId)) return;
              await rustTransform.run(sessionId, messages, output);
          }
        : async (): Promise<void> => {};

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
        // Deletion prunes per-session state so entries do not outlive the session.
        onSessionDeleted: (sessionId: string, directory?: string) => {
            addBoundedSession(deletedSessions, sessionId);
            if (directory && !sessionDirectoryBySession.has(sessionId)) {
                sessionDirectoryBySession.set(sessionId, directory);
            }
            if (rustMode || moduleClient.hasSessionRoute?.(sessionId)) {
                rustTransform.clearSession(sessionId);
            } else {
                moduleClient.closeSession?.(sessionId);
            }
            systemPromptHash.clearSession(sessionId);
            lastHeuristicsTurnId.delete(sessionId);
            clearToolPermissionDenied(sessionId);
            variantBySession.delete(sessionId);
            liveModelBySession.delete(sessionId);
            agentBySession.delete(sessionId);
            sessionDirectoryBySession.delete(sessionId);
            sessionMetadataReadStateBySession.delete(sessionId);
            internalChildSessions.delete(sessionId);
        },
    });

    const commandHandler = createEidnaraCommandHandler({
        moduleClient,
        compactionOff,
        resolveProjectRoot: sessionDirectoryFor,
        isSessionDeleted: (sessionId) => deletedSessions.has(sessionId),
        isSubagentSession,
        // The DB fallback gives /ctx-status the model-specific threshold before the first hook after a restart.
        getLiveModelKey: (sessionId) => {
            const model = resolveLiveModel(sessionId);
            return model ? `${model.providerID}/${model.modelID}` : undefined;
        },
        // /ctx-flush signals history rebuild, system-prompt adjuncts, and forced materialization.
        onFlush: (sessionId) => {
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
        sidekick: sidekickConfig
            ? {
                  config: sidekickConfig,
                  projectPath,
                  resolveSessionDirectory: sessionDirectoryFor,
                  client: deps.client,
                  language: deps.config.language,
              }
            : undefined,
    });

    const eventHook = createEventHook({
        eventHandler,
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
            liveModelBySession,
            variantBySession,
            agentBySession,
        }),
        event: eventHook,
        "command.execute.before": createCommandExecuteBeforeHook(commandHandler),
        "tool.execute.after": createToolExecuteAfterHook({
            subagentSessions,
            client: deps.client,
            transformMode: deps.config.transform_mode,
            todoStateSet: rustMode
                ? async ({ sessionId, stateJson, ownerMessageId }) => {
                      if (deletedSessions.has(sessionId) || subagentSessions.has(sessionId)) {
                          return undefined;
                      }
                      const projectRoot = await sessionDirectoryFor(sessionId);
                      // Session deletion and child classification may complete during the directory read.
                      if (deletedSessions.has(sessionId) || subagentSessions.has(sessionId)) {
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
                  }
                : undefined,
        }),
    };
    const hooksWithBackends = hooks as typeof hooks & {
        rustToolBackends?: RustToolBackends;
    };
    Object.defineProperty(hooksWithBackends, "rustToolBackends", {
        value: rustToolBackends,
        enumerable: false,
    });
    return hooksWithBackends;
}

export async function createEidnaraHookAsync(
    deps: EidnaraDeps,
): Promise<ReturnType<typeof createEidnaraHook>> {
    return createEidnaraHook(deps);
}
