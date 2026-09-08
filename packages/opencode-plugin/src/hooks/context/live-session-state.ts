import type { AgentBySession, LiveModelBySession, VariantBySession } from "./hook-handlers";

export interface LiveSessionState {
    liveModelBySession: LiveModelBySession;
    variantBySession: VariantBySession;
    agentBySession: AgentBySession;
    historyRefreshSessions: Set<string>;
    deferredHistoryRefreshSessions: Set<string>;
    systemPromptRefreshSessions: Set<string>;
    pendingMaterializationSessions: Set<string>;
    deferredMaterializationSessions: Set<string>;
    /** `sessionDirectoryBySession` caches `session.directory` values to avoid later SDK calls. */
    sessionDirectoryBySession: Map<string, string>;
    /**
     * `internalChildSessions` holds the ids of hidden `eidnara-` child sessions;
     * the transform and system-prompt hooks exempt them from the Eidnara pipeline.
     */
    internalChildSessions: Set<string>;
    /**
     * `subagentSessions` holds the ids of sessions created with a non-empty `parentID`,
     * so hooks can skip main-session-only work without a persisted session record.
     */
    subagentSessions: Set<string>;
}

/** Bounds retained child session IDs when deletion events are absent; matches the plugin's other per-session caps. */
const CHILD_SESSION_CAPACITY = 1000;

/** Adds `sessionId` and drops the oldest entry once the set is full; `Set` iterates in insertion order. */
export function addBoundedSession(sessions: Set<string>, sessionId: string): void {
    if (!sessions.has(sessionId) && sessions.size >= CHILD_SESSION_CAPACITY) {
        const oldest = sessions.values().next().value;
        if (oldest !== undefined) sessions.delete(oldest);
    }
    sessions.add(sessionId);
}

export function createLiveSessionState(): LiveSessionState {
    return {
        liveModelBySession: new Map<string, { providerID: string; modelID: string }>(),
        variantBySession: new Map<string, string | undefined>(),
        agentBySession: new Map<string, string>(),
        historyRefreshSessions: new Set<string>(),
        deferredHistoryRefreshSessions: new Set<string>(),
        systemPromptRefreshSessions: new Set<string>(),
        pendingMaterializationSessions: new Set<string>(),
        deferredMaterializationSessions: new Set<string>(),
        sessionDirectoryBySession: new Map<string, string>(),
        internalChildSessions: new Set<string>(),
        subagentSessions: new Set<string>(),
    };
}
