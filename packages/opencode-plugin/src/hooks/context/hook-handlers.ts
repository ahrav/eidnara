import { clearRustSessionStatus, clearWorkMetricsCarry } from "../../plugin/rpc-handlers";
import { clearSidebarSnapshotCache } from "../../plugin/sidebar-snapshot-cache";
import type { PluginContext } from "../../plugin/types";
import { sessionLog } from "../../shared/logger";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "../../shared/with-timeout";
import {
    cachedToolPermissionDenied,
    resolveTodowriteAvailability,
    todowritePermissionDenied,
} from "./ctx-reduce-availability";
import type { ContextUsageEntry } from "./event-handler";
import { getMessageUpdatedAssistantInfo, getSessionProperties } from "./event-payloads";
import { resolveSessionId as resolveEventSessionId } from "./event-resolvers";
import { clearIgnoredMessages, flushIgnoredMessages } from "./send-session-notification";
import { normalizeTodoStateJson } from "./todo-view";

export type LiveModelBySession = Map<string, { providerID: string; modelID: string }>;
export type VariantBySession = Map<string, string | undefined>;
export type AgentBySession = Map<string, string>;

/**
 * Three separate sets keep three independent lifetimes apart; one shared
 * flag would let defer passes blocked by an in-progress historian keep
 * re-firing the same flush signal across multiple turns. Each set has
 * exactly one consumer and one lifetime.
 *
 * Producers add each session to every set whose consumer must react.
 * Consumers drain their sets after consuming the signal.
 */

/**
 * A `HistoryRefreshSessions` entry requires rebuilding `<session-history>` on the next pass.
 * `<session-history>` contains compartments, facts, and memories in `message[0]`.
 * `prepareCompartmentInjection()` consumes `HistoryRefreshSessions` entries.
 * `prepareCompartmentInjection()` drains the entry after invocation, even when no rebuild occurs.
 *
 * `/ctx-flush`, real variant changes, and system-prompt hash changes add sessions to `HistoryRefreshSessions`.
 * Explicit flush, recomp, variant, and system-prompt-hash refresh paths add sessions to `HistoryRefreshSessions`.
 * Background historian/compressor publications use DeferredHistoryRefreshSessions.
 *
 * The background compressor does not add sessions to `HistoryRefreshSessions`.
 * The background compressor's output waits for the next natural cache-bust pass.
 */
export type HistoryRefreshSessions = Set<string>;

/** `DeferredHistoryRefreshSessions` persists history-refresh signals from background historian and compressor publications. */
export type DeferredHistoryRefreshSessions = Set<string>;

/**
 * A `SystemPromptRefreshSessions` entry requires re-reading system-prompt adjuncts from disk on the next system-transform call.
 * System-prompt adjuncts include project docs, the user profile, key files, and the sticky date.
 * `system-prompt-hash.ts` consumes `SystemPromptRefreshSessions` entries.
 * `system-prompt-hash.ts` drains each entry after refreshing.
 *
 * `/ctx-flush`, real variant changes, and system-prompt hash changes add sessions to `SystemPromptRefreshSessions`.
 *
 * Historian, compressor, and recomp do not add sessions to `SystemPromptRefreshSessions`.
 * Historian, compressor, and recomp do not change disk adjuncts, so re-reading them performs unnecessary I/O.
 */
export type SystemPromptRefreshSessions = Set<string>;

/**
 * A `PendingMaterializationSessions` entry requires queued `ctx_reduce` operations and heuristic cleanup to run.
 * The work remains pending when the current pass cannot safely run heuristics.
 * A compartment run prevents heuristic execution.
 * `transform-postprocess-phase.ts` drains entries only after `shouldRunHeuristics` executes.
 * `PendingMaterializationSessions` entries survive blocked passes until materialization succeeds.
 *
 * `/ctx-flush`, real variant changes, system-prompt hash changes, and explicit user refresh paths add sessions to `PendingMaterializationSessions`.
 * Background historian publications use DeferredMaterializationSessions.
 *
 * Historian and recomp queue drops via `queueDropsForCompartmentalizedMessages`; the next safe pass must materialize them to prevent context accumulation.
 */
export type PendingMaterializationSessions = Set<string>;

/** `DeferredMaterializationSessions` persists deferred drop-materialization signals from background historian publication. */
export type DeferredMaterializationSessions = Set<string>;

export type LastHeuristicsTurnId = Map<string, string>;

export function getLiveNotificationParams(
    sessionId: string,
    liveModelBySession: LiveModelBySession,
    variantBySession: VariantBySession,
    agentBySession?: AgentBySession,
    toastDurationMs?: number,
): {
    agent?: string;
    variant?: string;
    providerId?: string;
    modelId?: string;
    toastDurationMs?: number;
} {
    const model = liveModelBySession.get(sessionId);
    const variant = variantBySession.get(sessionId);
    const agent = agentBySession?.get(sessionId);
    return {
        ...(agent ? { agent } : {}),
        ...(variant ? { variant } : {}),
        ...(model ? { providerId: model.providerID, modelId: model.modelID } : {}),
        ...(typeof toastDurationMs === "number" ? { toastDurationMs } : {}),
    };
}

export function createChatMessageHook(args: {
    liveModelBySession: LiveModelBySession;
    variantBySession: VariantBySession;
    agentBySession: AgentBySession;
    upgradeReminder?: (sessionId: string) => Promise<void>;
}) {
    return async (input: {
        sessionID?: string;
        variant?: string;
        agent?: string;
        model?: { providerID?: string; modelID?: string };
    }) => {
        const sessionId = input.sessionID;
        if (!sessionId) return;

        if (args.upgradeReminder) {
            void args.upgradeReminder(sessionId);
        }

        if (input.model?.providerID && input.model.modelID) {
            args.liveModelBySession.set(sessionId, {
                providerID: input.model.providerID,
                modelID: input.model.modelID,
            });
        }

        args.variantBySession.set(sessionId, input.variant);
        if (input.agent) {
            args.agentBySession.set(sessionId, input.agent);
        }
    };
}

export function createEventHook(args: {
    eventHandler: (input: { event: { type: string; properties?: unknown } }) => Promise<void>;
    contextUsageMap: Map<string, ContextUsageEntry>;
    liveModelBySession: LiveModelBySession;
    variantBySession: VariantBySession;
    agentBySession: AgentBySession;
    /**
     * sessionDirectoryBySession caches resolved `session.directory` values from `client.session.get(...)`.
     * `session.deleted` clears `sessionDirectoryBySession` to prevent leaks.
     */
    sessionDirectoryBySession: Map<string, string>;
    /** `session.deleted` clears all signal sets to prevent leaks. */
    historyRefreshSessions: HistoryRefreshSessions;
    deferredHistoryRefreshSessions: DeferredHistoryRefreshSessions;
    systemPromptRefreshSessions: SystemPromptRefreshSessions;
    pendingMaterializationSessions: PendingMaterializationSessions;
    deferredMaterializationSessions: DeferredMaterializationSessions;
    lastHeuristicsTurnId: LastHeuristicsTurnId;
    commitSeenLastPass?: Map<string, boolean>;
    client: PluginContext["client"];
    protectedTags: number;
}) {
    return async (input: { event: { type: string; properties?: unknown } }) => {
        await args.eventHandler(input);

        if (input.event.type === "message.updated") {
            const assistantInfo = getMessageUpdatedAssistantInfo(input.event.properties);
            // An edit of an older response must not move the live model off the newest response; the event handler keeps the newest response's id in the usage entry. OpenCode message ids are time-ordered. commentlint: allow(JUDGE)
            const newestResponseId = args.contextUsageMap.get(
                assistantInfo?.sessionID ?? "",
            )?.messageID;
            const isOlderResponse =
                assistantInfo?.messageID !== undefined &&
                newestResponseId !== undefined &&
                assistantInfo.messageID < newestResponseId;
            if (assistantInfo?.providerID && assistantInfo?.modelID && !isOlderResponse) {
                args.liveModelBySession.set(assistantInfo.sessionID, {
                    providerID: assistantInfo.providerID,
                    modelID: assistantInfo.modelID,
                });
            }
        }

        const properties = getSessionProperties(input.event.properties);
        const sessionId = resolveEventSessionId(properties);
        if (!sessionId) return;

        if (input.event.type === "session.deleted") {
            args.liveModelBySession.delete(sessionId);
            args.variantBySession.delete(sessionId);
            args.agentBySession.delete(sessionId);
            args.sessionDirectoryBySession.delete(sessionId);
            args.historyRefreshSessions.delete(sessionId);
            args.deferredHistoryRefreshSessions.delete(sessionId);
            args.systemPromptRefreshSessions.delete(sessionId);
            args.pendingMaterializationSessions.delete(sessionId);
            args.deferredMaterializationSessions.delete(sessionId);
            args.lastHeuristicsTurnId.delete(sessionId);
            args.commitSeenLastPass?.delete(sessionId);
            clearIgnoredMessages(sessionId);
            clearSidebarSnapshotCache(sessionId);
            clearWorkMetricsCarry(sessionId);
            clearRustSessionStatus(sessionId);
        }

        if (input.event.type !== "session.deleted") {
            await flushIgnoredMessages(sessionId);
        }
    };
}

export function createCommandExecuteBeforeHook(commandHandler: {
    "command.execute.before": (
        input: import("./command-handler").CommandExecuteInput,
        output: import("./command-handler").CommandExecuteOutput,
        params: { agent?: string; variant?: string; providerId?: string; modelId?: string },
    ) => Promise<unknown>;
}) {
    return async (input: unknown, output: unknown) => {
        const typedInput = input as import("./command-handler").CommandExecuteInput & {
            agent?: string;
            variant?: string;
            providerID?: string;
            modelID?: string;
        };
        const params = {
            agent: typedInput.agent,
            variant: typedInput.variant,
            providerId: typedInput.providerID,
            modelId: typedInput.modelID,
        };
        return commandHandler["command.execute.before"](
            typedInput as import("./command-handler").CommandExecuteInput,
            output as import("./command-handler").CommandExecuteOutput,
            params,
        );
    };
}

export function createToolExecuteAfterHook(args: {
    /** Sessions created with a `parentID`; the hook skips task-list capture for them. */
    subagentSessions: ReadonlySet<string>;
    client?: PluginContext["client"];
    transformMode?: "ts" | "rust";
    todoStateSet?: (input: {
        sessionId: string;
        stateJson: string;
        ownerMessageId: string;
    }) => Promise<unknown>;
}) {
    return async (input: unknown) => {
        const typedInput = input as {
            tool?: string;
            sessionID?: string;
            args?: unknown;
            agent?: string;
        };
        if (!typedInput.sessionID || !typedInput.tool) {
            return;
        }

        await flushIgnoredMessages(typedInput.sessionID);

        if (typedInput.tool !== "todowrite") return;

        const todowriteVerdict = resolveTodowriteAvailability(typedInput.sessionID);
        if (todowriteVerdict.frozen && !todowriteVerdict.callable) return;
        const activeAgent = typedInput.agent;
        if (args.client) {
            try {
                if (
                    await withTimeout(
                        todowritePermissionDenied(args.client, typedInput.sessionID, activeAgent),
                        HOST_SDK_READ_TIMEOUT_MS,
                        "todowrite permission read timed out",
                    )
                ) {
                    return;
                }
            } catch (error) {
                // The permission check preserves a prior live deny across a transient or slow SDK read.
                // TODO: Prevent SDK read failures from resuming stale capture.
                if (cachedToolPermissionDenied(typedInput.sessionID, "todowrite")) {
                    return;
                }
                sessionLog(
                    typedInput.sessionID,
                    "todowrite permission read failed during capture (ignored):",
                    error,
                );
            }
        }
        if (args.subagentSessions.has(typedInput.sessionID)) return;
        const todoArgs = typedInput.args as { todos?: unknown } | undefined;
        const todos = todoArgs?.todos;
        if (!Array.isArray(todos)) return;
        const normalizedTodos = normalizeTodoStateJson(todos);
        if (normalizedTodos === null) return;
        if (args.transformMode !== "rust" || !args.todoStateSet) return;

        const todoSessionId = typedInput.sessionID;
        const rawArgs =
            typedInput.args && typeof typedInput.args === "object"
                ? (typedInput.args as Record<string, unknown>)
                : {};
        const ownerMessageId =
            (typeof rawArgs.owner_message_id === "string" && rawArgs.owner_message_id) ||
            (typeof rawArgs.message_id === "string" && rawArgs.message_id) ||
            (typeof (typedInput as { messageID?: unknown }).messageID === "string" &&
                (typedInput as { messageID: string }).messageID) ||
            (typeof (typedInput as { callID?: unknown }).callID === "string" &&
                (typedInput as { callID: string }).callID) ||
            typedInput.sessionID;
        void args
            .todoStateSet({
                sessionId: todoSessionId,
                stateJson: normalizedTodos,
                ownerMessageId,
            })
            .catch((error) => {
                sessionLog(todoSessionId, "rust todo_state.set failed (ignored):", error);
            });
    };
}
