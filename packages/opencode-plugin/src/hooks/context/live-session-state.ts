import { BoundedSessionMap } from "../../shared/bounded-session-map";
import type { ContextUsageEntry } from "./event-handler";
import type { AgentBySession, LiveModelBySession, VariantBySession } from "./hook-handlers";

/** Sessions whose live usage stays resident. Each entry is the session's newest-response record; `session.deleted` clears it, and an evicted session is re-read from OpenCode's database on the next poll. Matches the sticky sidebar cache's session cap. commentlint: allow(JUDGE) */
export const MAX_LIVE_USAGE_SESSIONS = 100;

export interface SessionMetadataReadState {
    attempts: number;
    retryAfterMs: number;
    inFlight?: Promise<{
        directory?: unknown;
        parentID?: unknown;
        title?: unknown;
    } | null>;
}

export interface LiveSessionState {
    liveModelBySession: LiveModelBySession;
    variantBySession: VariantBySession;
    agentBySession: AgentBySession;
    /** `contextUsageBySession` holds each session's input-token usage from its latest assistant response; the sidebar reads it when the daemon supplies no usage. commentlint: allow(JUDGE) */
    contextUsageBySession: BoundedSessionMap<ContextUsageEntry>;
    historyRefreshSessions: Set<string>;
    deferredHistoryRefreshSessions: Set<string>;
    systemPromptRefreshSessions: Set<string>;
    pendingMaterializationSessions: Set<string>;
    deferredMaterializationSessions: Set<string>;
    /** `sessionDirectoryBySession` caches `session.directory` values to avoid later SDK calls. */
    sessionDirectoryBySession: Map<string, string>;
    /** `retryAfterMs` prevents one hook turn from spending both bounded metadata reads. commentlint: allow(JUDGE) */
    sessionMetadataReadStateBySession: Map<string, SessionMetadataReadState>;
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
    /**
     * `staleDaemonUsageSessions` holds sessions the host compacted after the daemon last received
     * usage; the sidebar reads live usage for them until a transform forwards a post-compaction sample.
     */
    staleDaemonUsageSessions: Set<string>;
}

/** Hidden Eidnara child sessions carry this title prefix at creation. */
export const INTERNAL_CHILD_TITLE_PREFIX = "eidnara-";

/** A session the host reports with a parent is a subagent; one whose title carries the internal prefix is also a hidden Eidnara child. */
export function recordChildSession(
    sets: { subagentSessions?: Set<string>; internalChildSessions?: Set<string> },
    sessionId: string,
    session: { parentID?: unknown; title?: unknown },
): { internalChild: boolean } {
    const isChild = typeof session.parentID === "string" && session.parentID.length > 0;
    if (!isChild) return { internalChild: false };
    if (sets.subagentSessions) addBoundedSession(sets.subagentSessions, sessionId);
    const internalChild =
        typeof session.title === "string" && session.title.startsWith(INTERNAL_CHILD_TITLE_PREFIX);
    if (internalChild && sets.internalChildSessions) {
        addBoundedSession(sets.internalChildSessions, sessionId);
    }
    return { internalChild };
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
        contextUsageBySession: new BoundedSessionMap<ContextUsageEntry>(MAX_LIVE_USAGE_SESSIONS),
        historyRefreshSessions: new Set<string>(),
        deferredHistoryRefreshSessions: new Set<string>(),
        systemPromptRefreshSessions: new Set<string>(),
        pendingMaterializationSessions: new Set<string>(),
        deferredMaterializationSessions: new Set<string>(),
        sessionDirectoryBySession: new Map<string, string>(),
        sessionMetadataReadStateBySession: new Map(),
        internalChildSessions: new Set<string>(),
        subagentSessions: new Set<string>(),
        staleDaemonUsageSessions: new Set<string>(),
    };
}
