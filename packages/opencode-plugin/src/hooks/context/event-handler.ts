import { detectOverflow } from "../../features/context/overflow-detection";
import { log, sessionLog } from "../../shared/logger";
import { refreshModelLimitsAfterAuthOnce } from "../../shared/models-dev-cache";
import { removeCompactionMarkerForSession } from "./compaction-marker-manager";
import {
    type ContextUsage,
    getMessageRemovedInfo,
    getMessageUpdatedAssistantInfo,
    getSessionCreatedInfo,
    getSessionErrorInfo,
    getSessionProperties,
} from "./event-payloads";
import { resolveContextLimit, resolveSessionId } from "./event-resolvers";
import { invalidateTrueRawTokenCache } from "./read-session-true-raw-tokens";

const CONTEXT_USAGE_TTL_MS = 60 * 60 * 1000;

export interface ContextUsageEntry {
    usage: ContextUsage;
    updatedAt: number;
    lastResponseTime?: number;
    hasUsageTokens?: boolean;
    /** The model whose window `usage` was measured against; readers must not pair it with another model's limit. */
    model?: { providerID: string; modelID: string };
}

export interface EventHandlerDeps {
    contextUsageMap: Map<string, ContextUsageEntry>;
    onSessionCacheInvalidated?: (sessionId: string) => void;
    onRustWireInvalidated?: (sessionId: string) => void;
    onSessionDeleted?: (sessionId: string) => void;
    /** The in-process client OpenCode hands the plugin; the post-auth model-limit re-warm reads provider metadata through it. */
    client?: unknown;
    /**
     * `internalChildSessions` tracks `eidnara-` child sessions so transform and system-prompt hooks exempt them from the Eidnara pipeline.
     */
    internalChildSessions?: Set<string>;
    /** `subagentSessions` records every session created with a non-empty `parentID`, in memory only. */
    subagentSessions?: Set<string>;
}

/** Hidden Eidnara child sessions carry this title prefix at creation. */
const INTERNAL_CHILD_TITLE_PREFIX = "eidnara-";

function evictExpiredUsageEntries(contextUsageMap: Map<string, ContextUsageEntry>): void {
    const now = Date.now();
    for (const [sessionId, entry] of contextUsageMap) {
        if (now - entry.updatedAt > CONTEXT_USAGE_TTL_MS) {
            contextUsageMap.delete(sessionId);
        }
    }
}

/** An overflow error means the host will rebuild the window, so the session's injection cache is stale. */
function invalidateOnOverflow(
    deps: EventHandlerDeps,
    sessionId: string,
    error: unknown,
    via: string,
): void {
    const detection = detectOverflow(error);
    if (!detection.isOverflow) {
        return;
    }
    sessionLog(
        sessionId,
        `overflow detected via ${via}: reportedLimit=${detection.reportedLimit ?? "unknown"} provenance=${detection.reportedLimitProvenance ?? "n/a"} pattern=${detection.matchedPattern ?? "n/a"}`,
    );
    deps.onSessionCacheInvalidated?.(sessionId);
}

export function createEventHandler(deps: EventHandlerDeps) {
    return async (input: { event: { type: string; properties?: unknown } }): Promise<void> => {
        evictExpiredUsageEntries(deps.contextUsageMap);

        const properties = getSessionProperties(input.event.properties);

        if (input.event.type === "session.created") {
            const info = getSessionCreatedInfo(input.event.properties);
            if (!info) {
                return;
            }

            const isChild = info.parentID.length > 0;
            if (isChild) {
                deps.subagentSessions?.add(info.id);
            }

            // The handler adds hidden sessions titled `eidnara-` to `internalChildSessions` so transform and system-prompt hooks exempt them; the set is not persisted across restarts.
            if (
                deps.internalChildSessions &&
                isChild &&
                typeof info.title === "string" &&
                info.title.startsWith(INTERNAL_CHILD_TITLE_PREFIX)
            ) {
                deps.internalChildSessions.add(info.id);
                sessionLog(
                    info.id,
                    `marked internal eidnara child (title="${info.title}") — exempt from transform + injection`,
                );
            }
            return;
        }

        if (input.event.type === "session.error") {
            const errInfo = getSessionErrorInfo(input.event.properties);
            if (!errInfo) {
                return;
            }
            try {
                invalidateOnOverflow(deps, errInfo.sessionID, errInfo.error, "session.error");
            } catch (error) {
                sessionLog(errInfo.sessionID, "event session.error handling failed:", error);
            }
            return;
        }

        if (input.event.type === "message.updated") {
            const info = getMessageUpdatedAssistantInfo(input.event.properties);
            if (!info) {
                const sessionId = properties ? resolveSessionId(properties) : null;
                if (sessionId) {
                    sessionLog(
                        sessionId,
                        "event message.updated: no assistant info extracted from event",
                    );
                } else {
                    log("[eidnara] event message.updated: no assistant info extracted from event");
                }
                return;
            }

            // Streaming, edited, or retried messages carry stale cached token estimates; a missing message ID widens the invalidation to the whole session.
            invalidateTrueRawTokenCache({
                sessionId: info.sessionID,
                messageId: info.messageID,
                reason: "message.updated",
            });

            // OpenCode may report overflow through `session.error` or the assistant message error; either can arrive first or be absent.
            if (info.error !== undefined && info.error !== null) {
                try {
                    invalidateOnOverflow(deps, info.sessionID, info.error, "message.updated");
                } catch (error) {
                    sessionLog(
                        info.sessionID,
                        "event message.updated overflow handling failed:",
                        error,
                    );
                }
            }

            const now = Date.now();
            const usageTokens = [
                info.tokens?.input,
                info.tokens?.cache?.read,
                info.tokens?.cache?.write,
            ];
            const hasUsageTokens = usageTokens.some(
                (value) => typeof value === "number" && value > 0,
            );

            sessionLog(
                info.sessionID,
                `event message.updated: provider=${info.providerID} model=${info.modelID} hasUsageTokens=${hasUsageTokens} tokens.input=${info.tokens?.input} cache.read=${info.tokens?.cache?.read} cache.write=${info.tokens?.cache?.write}`,
            );

            if (!hasUsageTokens) {
                sessionLog(info.sessionID, "event message.updated: skipping — no usage tokens");
                return;
            }

            try {
                const totalInputTokens =
                    (info.tokens?.input ?? 0) +
                    (info.tokens?.cache?.read ?? 0) +
                    (info.tokens?.cache?.write ?? 0);
                // The model-limit cache re-warms once after the first usage-bearing response so authenticated limits replace the startup catalog values.
                if (deps.client) {
                    await refreshModelLimitsAfterAuthOnce(
                        deps.client as Parameters<typeof refreshModelLimitsAfterAuthOnce>[0],
                    );
                }
                const contextLimit = resolveContextLimit(info.providerID, info.modelID);
                const percentage = contextLimit > 0 ? (totalInputTokens / contextLimit) * 100 : 0;

                sessionLog(
                    info.sessionID,
                    `event message.updated: totalInputTokens=${totalInputTokens} contextLimit=${contextLimit} percentage=${percentage.toFixed(1)}%`,
                );

                deps.contextUsageMap.set(info.sessionID, {
                    usage: {
                        percentage,
                        inputTokens: totalInputTokens,
                    },
                    updatedAt: now,
                    lastResponseTime: now,
                    hasUsageTokens: true,
                    model:
                        info.providerID && info.modelID
                            ? { providerID: info.providerID, modelID: info.modelID }
                            : undefined,
                });
            } catch (error) {
                sessionLog(info.sessionID, "event message.updated usage tracking failed:", error);
            }
            return;
        }

        if (input.event.type === "message.removed") {
            const info = getMessageRemovedInfo(input.event.properties);
            if (!info) {
                const sessionId = properties ? resolveSessionId(properties) : null;
                if (sessionId) {
                    sessionLog(
                        sessionId,
                        "event message.removed: no message removal info extracted from event",
                    );
                } else {
                    log(
                        "[eidnara] event message.removed: no message removal info extracted from event",
                    );
                }
                return;
            }

            deps.onRustWireInvalidated?.(info.sessionID);
            sessionLog(
                info.sessionID,
                `event message.removed: invalidating state for message ${info.messageID}`,
            );

            try {
                // Marker removal matches only plugin-owned rows by signature and is idempotent, so it runs on every removal without knowing whether the removed message was the marker boundary.
                removeCompactionMarkerForSession(info.sessionID);

                invalidateTrueRawTokenCache({
                    sessionId: info.sessionID,
                    messageId: info.messageID,
                    reason: "message.removed",
                });

                deps.onSessionCacheInvalidated?.(info.sessionID);
                sessionLog(
                    info.sessionID,
                    "event message.removed: cleared session injection cache",
                );
            } catch (error) {
                sessionLog(info.sessionID, "event message.removed cleanup failed:", error);
            }
            return;
        }

        if (input.event.type === "session.compacted") {
            const sessionId = resolveSessionId(properties);
            if (!sessionId) {
                return;
            }

            // Native compaction deletes the boundary message, so the marker rows would otherwise be orphaned.
            try {
                removeCompactionMarkerForSession(sessionId);
            } catch (error) {
                sessionLog(sessionId, "event session.compacted marker cleanup failed:", error);
            }
            invalidateTrueRawTokenCache({ sessionId, reason: "session.compacted" });
            deps.onSessionCacheInvalidated?.(sessionId);
            return;
        }

        if (input.event.type === "session.deleted") {
            const sessionId = resolveSessionId(properties);
            if (!sessionId) {
                return;
            }

            try {
                removeCompactionMarkerForSession(sessionId);
            } catch (error) {
                sessionLog(sessionId, "event session.deleted marker cleanup failed:", error);
            }
            deps.onSessionCacheInvalidated?.(sessionId);
            deps.onSessionDeleted?.(sessionId);
            deps.contextUsageMap.delete(sessionId);
            deps.subagentSessions?.delete(sessionId);
            deps.internalChildSessions?.delete(sessionId);
            invalidateTrueRawTokenCache({ sessionId, reason: "session.deleted" });
            return;
        }
    };
}
