/**
 *
 *
 *
 *     real state.
 *
 */
import { BoundedSessionMap } from "../shared/bounded-session-map";
import type { SidebarSnapshot } from "../shared/rpc-types";

interface CachedSnapshot {
    snapshot: SidebarSnapshot;
    cachedAt: number;
}

const MAX_CACHED_SESSIONS = 100;
/** Roots retained per session. A session lives under one root; the slack covers alternate spellings of that root without letting a long-lived session accumulate a snapshot per directory it was ever polled from. commentlint: allow(JUDGE) */
export const MAX_CACHED_ROOTS_PER_SESSION = 4;
const STALE_SNAPSHOT_AGE_MS = 5 * 60 * 1000; // 5 minutes

// The daemon scopes session state by project root, so a session polled under two roots needs one sticky snapshot per root. Both levels are LRU-bounded.
const cache = new BoundedSessionMap<BoundedSessionMap<CachedSnapshot>>(MAX_CACHED_SESSIONS);

function peekCached(sessionId: string, directory: string): CachedSnapshot | undefined {
    return cache.peek(sessionId)?.peek(directory);
}

function storeCached(sessionId: string, directory: string, entry: CachedSnapshot): void {
    const byRoot =
        cache.get(sessionId) ?? new BoundedSessionMap<CachedSnapshot>(MAX_CACHED_ROOTS_PER_SESSION);
    byRoot.set(directory, entry);
    cache.set(sessionId, byRoot);
}

function dropCached(sessionId: string, directory: string): void {
    const byRoot = cache.peek(sessionId);
    if (!byRoot) return;
    byRoot.delete(directory);
    if (byRoot.size === 0) cache.delete(sessionId);
}

/**
 *
 */
export function applyStickySnapshotCache(
    sessionId: string,
    directory: string,
    fresh: SidebarSnapshot,
): SidebarSnapshot {
    const now = Date.now();

    if (fresh.inputTokens > 0) {
        storeCached(sessionId, directory, { snapshot: fresh, cachedAt: now });
        return fresh;
    }

    const cached = peekCached(sessionId, directory);
    if (!cached) {
        return fresh;
    }
    if (now - cached.cachedAt > STALE_SNAPSHOT_AGE_MS) {
        dropCached(sessionId, directory);
        return fresh;
    }
    //
    //
    const stateSurvived =
        fresh.compartmentCount >= cached.snapshot.compartmentCount &&
        fresh.memoryCount >= cached.snapshot.memoryCount;
    if (!hasInFlightEvidence(fresh) && !stateSurvived) {
        dropCached(sessionId, directory);
        return fresh;
    }

    // stale counts.
    return {
        ...fresh,
        usagePercentage: cached.snapshot.usagePercentage,
        inputTokens: cached.snapshot.inputTokens,
        systemPromptTokens: cached.snapshot.systemPromptTokens,
        compartmentTokens: cached.snapshot.compartmentTokens,
        factTokens: cached.snapshot.factTokens,
        memoryTokens: cached.snapshot.memoryTokens,
        conversationTokens: cached.snapshot.conversationTokens,
        toolCallTokens: cached.snapshot.toolCallTokens,
        toolDefinitionTokens: cached.snapshot.toolDefinitionTokens,
    };
}

function hasInFlightEvidence(snapshot: SidebarSnapshot): boolean {
    return (
        snapshot.compartmentInProgress || snapshot.historianRunning || snapshot.pendingOpsCount > 0
    );
}

export function clearSidebarSnapshotCache(sessionId: string): void {
    cache.delete(sessionId);
}

export function resetSidebarSnapshotCache(): void {
    cache.clear();
}
