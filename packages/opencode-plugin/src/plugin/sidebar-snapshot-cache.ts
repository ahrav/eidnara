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
    modelKey: string | undefined;
}

/** Identifies one sticky snapshot. Token totals are measured against one model's window, so a different `modelKey` resets the entry instead of reusing it. commentlint: allow(JUDGE) */
export interface StickySnapshotScope {
    sessionId: string;
    directory: string;
    modelKey?: string;
}

const MAX_CACHED_SESSIONS = 100;
/** Roots retained per session. A session lives under one root; the slack covers alternate spellings of that root without letting a long-lived session accumulate a snapshot per directory it was ever polled from. commentlint: allow(JUDGE) */
export const MAX_CACHED_ROOTS_PER_SESSION = 4;
const STALE_SNAPSHOT_AGE_MS = 5 * 60 * 1000; // 5 minutes

// The daemon scopes session state by project root, so a session polled under two roots needs one sticky snapshot per root. Both levels are LRU-bounded.
const cache = new BoundedSessionMap<BoundedSessionMap<CachedSnapshot>>(MAX_CACHED_SESSIONS);

function peekCached(scope: StickySnapshotScope): CachedSnapshot | undefined {
    return cache.peek(scope.sessionId)?.peek(scope.directory);
}

function storeCached(scope: StickySnapshotScope, entry: CachedSnapshot): void {
    const byRoot =
        cache.get(scope.sessionId) ??
        new BoundedSessionMap<CachedSnapshot>(MAX_CACHED_ROOTS_PER_SESSION);
    byRoot.set(scope.directory, entry);
    cache.set(scope.sessionId, byRoot);
}

function dropCached(scope: StickySnapshotScope): void {
    const byRoot = cache.peek(scope.sessionId);
    if (!byRoot) return;
    byRoot.delete(scope.directory);
    if (byRoot.size === 0) cache.delete(scope.sessionId);
}

/**
 *
 */
export function applyStickySnapshotCache(
    scope: StickySnapshotScope,
    fresh: SidebarSnapshot,
): SidebarSnapshot {
    const now = Date.now();

    if (fresh.inputTokens > 0) {
        storeCached(scope, { snapshot: fresh, cachedAt: now, modelKey: scope.modelKey });
        return fresh;
    }

    const cached = peekCached(scope);
    if (!cached) {
        return fresh;
    }
    if (now - cached.cachedAt > STALE_SNAPSHOT_AGE_MS || cached.modelKey !== scope.modelKey) {
        dropCached(scope);
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
        dropCached(scope);
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
