/**
 * TUI data layer — pure RPC client, no direct SQLite access.
 */
import { getEidnaraStorageDir } from "../../shared/data-path";
import { EidnaraRpcClient } from "../../shared/rpc-client";
import type { SidebarSnapshot, StatusDetail } from "../../shared/rpc-types";

export type { SidebarSnapshot, StatusDetail };

let rpcClient: EidnaraRpcClient | null = null;
let rpcGeneration = 0;

/** Initialize the RPC client. Call once on TUI startup. */
export function initRpcClient(directory: string): void {
    const storageDir = getEidnaraStorageDir();
    // Bump the generation before replacing the client so late notification
    // responses from a disposed client are ignored (the WS socket observes the
    // new generation and abandons its in-flight connect).
    rpcGeneration += 1;
    rpcClient = new EidnaraRpcClient(storageDir, directory);
}

export function getRpcGeneration(): number {
    return rpcGeneration;
}

/**
 * */
export function getRpcClient(): EidnaraRpcClient | null {
    return rpcClient;
}

/* */
export function closeRpc(): void {
    rpcGeneration += 1;
    rpcClient?.reset();
    rpcClient = null;
}

function isRpcError(value: unknown): boolean {
    return value !== null && typeof value === "object" && "error" in value;
}

const EMPTY_SNAPSHOT: SidebarSnapshot = {
    sessionId: "",
    usagePercentage: 0,
    inputTokens: 0,
    contextLimit: 0,
    systemPromptTokens: 0,
    compartmentCount: 0,
    memoryCount: 0,
    memoryState: null,
    memoryBlockCount: 0,
    pendingOpsCount: 0,
    historianRunning: false,
    compartmentInProgress: false,
    sessionNoteCount: 0,
    readySmartNoteCount: 0,
    cacheTtl: "5m",
    lastTransformError: null,
    lastDreamerRunAt: null,
    projectIdentity: null,
    compartmentTokens: 0,
    factTokens: 0,
    memoryTokens: 0,
    docsTokens: 0,
    profileTokens: 0,
    conversationTokens: 0,
    toolCallTokens: 0,
    toolDefinitionTokens: 0,
    executeThreshold: 65,
    newWorkTokens: null,
    totalInputTokens: null,
};

/**
 * The client caches usable snapshots by session when RPC cannot return one.
 * - RPC call fails before a usable snapshot is received (timeout, abort, parse error)
 *   - Server returns an error envelope
 *
 * The 5-minute staleness ceiling prevents old data after long disconnects.
 */
interface CachedSnapshot {
    snapshot: SidebarSnapshot;
    cachedAt: number;
}
const STICKY_TTL_MS = 5 * 60 * 1000;
const STICKY_MAX_ENTRIES = 100;
const stickySidebarCache = new Map<string, CachedSnapshot>();

/** The producer resolves a snapshot from both the session and the requested project root, so the cache key carries both. */
function stickyKey(sessionId: string, directory: string): string {
    return `${sessionId}\u001f${directory}`;
}

function rememberSidebarSnapshot(snapshot: SidebarSnapshot, directory: string): void {
    if (!snapshot.sessionId) return;
    const key = stickyKey(snapshot.sessionId, directory);
    // The entry cap prevents unbounded growth across session switches.
    if (stickySidebarCache.size >= STICKY_MAX_ENTRIES && !stickySidebarCache.has(key)) {
        const firstKey = stickySidebarCache.keys().next().value;
        if (firstKey) stickySidebarCache.delete(firstKey);
    }
    stickySidebarCache.set(key, {
        snapshot,
        cachedAt: Date.now(),
    });
}

function recallSidebarSnapshot(
    sessionId: string,
    directory: string,
    fallback: SidebarSnapshot,
): SidebarSnapshot {
    const key = stickyKey(sessionId, directory);
    const cached = stickySidebarCache.get(key);
    if (!cached) return fallback;
    if (Date.now() - cached.cachedAt > STICKY_TTL_MS) {
        stickySidebarCache.delete(key);
        return fallback;
    }
    return cached.snapshot;
}

/* */
export async function loadSidebarSnapshot(
    sessionId: string,
    directory: string,
): Promise<SidebarSnapshot> {
    const empty: SidebarSnapshot = { ...EMPTY_SNAPSHOT, sessionId };
    if (!rpcClient) return recallSidebarSnapshot(sessionId, directory, empty);
    try {
        const result = await rpcClient.call<SidebarSnapshot>("sidebar-snapshot", {
            sessionId,
            directory,
        });
        if (isRpcError(result)) {
            return recallSidebarSnapshot(sessionId, directory, empty);
        }
        // Every successful snapshot, including a zero-token one, becomes the newest cached value so a later failure replays current memory counts rather than EMPTY_SNAPSHOT.
        rememberSidebarSnapshot(result, directory);
        return result;
    } catch {
        return recallSidebarSnapshot(sessionId, directory, empty);
    }
}

/* */
export async function loadStatusDetail(
    sessionId: string,
    directory: string,
    modelKey?: string,
): Promise<StatusDetail> {
    const emptyDetail: StatusDetail = {
        ...EMPTY_SNAPSHOT,
        sessionId,
        tagCounter: 0,
        activeTags: 0,
        droppedTags: 0,
        totalTags: 0,
        activeBytes: 0,
        lastResponseTime: 0,
        lastNudgeTokens: 0,
        lastTransformError: null,
        isSubagent: false,
        pendingOps: [],
        contextLimit: 0,
        cacheTtlMs: 0,
        cacheRemainingMs: 0,
        cacheExpired: false,
        cacheNeverExpires: false,
        executeThreshold: 65,
        executeThresholdMode: "percentage",
        protectedTagCount: 20,
        historyBudgetPercentage: 0.15,
        historyBlockTokens: 0,
        compressionBudget: null,
        compressionUsage: null,
        toastDurationMs: 5000,
        loggerDiagnostics: {
            swallowedWriteCount: 0,
            lastErrorMessage: null,
            lastErrorTime: null,
        },
    };

    if (!rpcClient) return emptyDetail;
    try {
        const result = await rpcClient.call<StatusDetail>("status-detail", {
            sessionId,
            directory,
            modelKey,
        });
        if (isRpcError(result)) {
            return emptyDetail;
        }
        return result;
    } catch {
        return emptyDetail;
    }
}

export async function loadToastDurationMs(): Promise<number> {
    if (!rpcClient) return 5000;
    try {
        const result = await rpcClient.call<{ toastDurationMs?: number }>("toast-duration", {});
        return typeof result.toastDurationMs === "number" ? result.toastDurationMs : 5000;
    } catch {
        return 5000;
    }
}

/**
 */
