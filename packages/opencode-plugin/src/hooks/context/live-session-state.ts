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
