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
const STALE_SNAPSHOT_AGE_MS = 5 * 60 * 1000; // 5 minutes

const cache = new BoundedSessionMap<CachedSnapshot>(MAX_CACHED_SESSIONS);

/**
 *
 */
export function applyStickySnapshotCache(
    sessionId: string,
    fresh: SidebarSnapshot,
): SidebarSnapshot {
    const now = Date.now();

    if (fresh.inputTokens > 0) {
        cache.set(sessionId, { snapshot: fresh, cachedAt: now });
        return fresh;
    }

    const cached = cache.peek(sessionId);
    if (!cached) {
        return fresh;
    }
    if (now - cached.cachedAt > STALE_SNAPSHOT_AGE_MS) {
        cache.delete(sessionId);
        return fresh;
    }
    // A memory count is evidence of a reset only when the read behind it is complete:
    // a non-`available` state carries no rows, and a truncated read is a lower bound,
    // so a decrease in either case does not prove state was deleted.
    const memoryCountIsEvidence =
        fresh.memoryState === "available" && fresh.memoryTruncated !== true;
    const stateSurvived =
        fresh.compartmentCount >= cached.snapshot.compartmentCount &&
        (!memoryCountIsEvidence || fresh.memoryCount >= cached.snapshot.memoryCount);
    if (!hasInFlightEvidence(fresh) && !stateSurvived) {
        cache.delete(sessionId);
        return fresh;
    }

    // Cached token fields restore one internally consistent token breakdown while
    // live state, counts, and cumulative work metrics remain fresh.
    return {
        ...fresh,
        usagePercentage: cached.snapshot.usagePercentage,
        native_context_usage_percentage: cached.snapshot.native_context_usage_percentage,
        inputTokens: cached.snapshot.inputTokens,
        systemPromptTokens: cached.snapshot.systemPromptTokens,
        compartmentTokens: cached.snapshot.compartmentTokens,
        factTokens: cached.snapshot.factTokens,
        memoryTokens: cached.snapshot.memoryTokens,
        docsTokens: cached.snapshot.docsTokens,
        profileTokens: cached.snapshot.profileTokens,
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
