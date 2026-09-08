import { isCompactionEnabled } from "../config/agent-disable";
import type { EidnaraConfig } from "../config/schema/eidnara";
import {
    resolveProjectIdentity,
    resolveProjectRootDirectory,
} from "../features/context/project-identity";
import {
    computeOpenCodeWorkMetricsIncremental,
    emptyWorkMetricsCarry,
    type WorkMetricsCarry,
} from "../features/context/work-metrics";
import {
    DEFAULT_CACHE_TTL_MS,
    parseCacheTtlMs,
    resolveCacheTtl,
    resolveContextLimit,
    resolveContextWindowGeometry,
    resolveExecuteThresholdDetail,
} from "../hooks/context/event-resolvers";
import { kernelClientResolver } from "../hooks/context/kernel-transport";
import type { LiveSessionState } from "../hooks/context/live-session-state";
import {
    findLastAssistantModelFromOpenCodeDb,
    openCodeDbExists,
    withReadOnlySessionDb,
} from "../hooks/context/read-session-db";
import type { RustModeModuleClient } from "../hooks/context/rust-mode-transform";
import { calibrateBuckets, resolveModelCalibration } from "../hooks/context/tokenizer-calibration";
import { BoundedSessionMap } from "../shared/bounded-session-map";
import {
    disabled,
    isServedMemoryDecisionRow,
    type KernelMemorySnapshot,
    kernelMemorySnapshotFrom,
    stateKey,
} from "../shared/kernel-client";
import { getLoggerDiagnostics, log } from "../shared/logger";
import type { EidnaraRpcServer } from "../shared/rpc-server";
import type { SidebarSnapshot, StatusDetail } from "../shared/rpc-types";
import {
    resolveTailHygieneStatus,
    type WireTailHygieneBaseline,
} from "../shared/tail-hygiene-status";
import { applyStickySnapshotCache } from "./sidebar-snapshot-cache";

/** Sessions whose work-metrics carry stays resident. Matches the sticky sidebar cache's session cap, since both hold one entry per polled session. commentlint: allow(JUDGE) */
const WORK_METRICS_CARRY_MAX_SESSIONS = 100;
// Each poll processes only assistant rows newer than its watermark because the long-lived RPC server retains each session's carry across polls.
// Losing a carry is safe: a restart, an LRU eviction, and `session.deleted` all make the next poll re-read that session's assistant rows from the start.
const workMetricsCarryBySession = new BoundedSessionMap<WorkMetricsCarry>(
    WORK_METRICS_CARRY_MAX_SESSIONS,
);
const RUST_STATUS_CACHE_TTL_MS = 2_000;
/** Live entries per poll cache. Each open sidebar pane polls one `(session, directory)` pair, so the cap covers concurrent panes while bounding growth across sessions and projects. commentlint: allow(JUDGE) */
const POLL_CACHE_MAX_ENTRIES = 32;

export function clearWorkMetricsCarry(sessionId: string): void {
    workMetricsCarryBySession.delete(sessionId);
}

/**
 * The module transport routes by `(sessionId, projectRoot)`, so every per-poll cache keys by both:
 * one session id polled under two project roots must not share a daemon answer.
 */
function pollCacheKey(sessionId: string, directory: string): string {
    return `${sessionId}\u001f${directory}`;
}

/** Every `get` and `set` sweeps expired entries, and a full cache evicts its oldest entry before inserting, so a long-lived RPC server polling many sessions never accumulates dead snapshots. commentlint: allow(JUDGE) */
export class BoundedTtlCache<V> {
    private readonly entries = new Map<string, { value: V; cachedAt: number }>();

    constructor(
        private readonly ttlMs: number,
        private readonly maxEntries: number,
    ) {}

    private sweep(nowMs: number): void {
        for (const [key, entry] of this.entries) {
            if (nowMs - entry.cachedAt >= this.ttlMs) this.entries.delete(key);
        }
    }

    get(key: string, nowMs = Date.now()): V | undefined {
        this.sweep(nowMs);
        return this.entries.get(key)?.value;
    }

    set(key: string, value: V, nowMs = Date.now()): void {
        this.sweep(nowMs);
        this.entries.delete(key);
        if (this.entries.size >= this.maxEntries) {
            let oldestKey: string | undefined;
            let oldestAt = Number.POSITIVE_INFINITY;
            for (const [candidate, entry] of this.entries) {
                if (entry.cachedAt < oldestAt) {
                    oldestAt = entry.cachedAt;
                    oldestKey = candidate;
                }
            }
            if (oldestKey !== undefined) this.entries.delete(oldestKey);
        }
        this.entries.set(key, { value, cachedAt: nowMs });
    }

    get size(): number {
        return this.entries.size;
    }
}

export interface RustSessionStatus {
    usage?: { current_total_input_tokens?: number; context_limit_tokens?: number };
    tail_hygiene?: WireTailHygieneBaseline | null;
    boundary_present?: boolean;
    coverage_ordinal?: number | null;
    compartment_count?: number;
    compartment_tokens?: number;
    pending_drop_count?: number;
    tag_count?: number;
    pending_m1_delta?: boolean;
    pending_m1_age_ms?: number | null;
    wrapup_active?: boolean;
    wrapup_rounds?: number | null;
    pass_trace?: { last_reject_error?: string | null } | null;
}
const rustStatusCache = new BoundedTtlCache<RustSessionStatus>(
    RUST_STATUS_CACHE_TTL_MS,
    POLL_CACHE_MAX_ENTRIES,
);

/**
 * When OpenCode's DB is unavailable or unreadable, the sidebar reports zero work metrics.
 */
function resolveSidebarWorkMetrics(sessionId: string): {
    newWorkTokens: number;
    totalInputTokens: number;
} {
    if (!openCodeDbExists()) {
        return { newWorkTokens: 0, totalInputTokens: 0 };
    }
    try {
        const carry = workMetricsCarryBySession.get(sessionId) ?? emptyWorkMetricsCarry();
        const { carry: nextCarry, metrics } = withReadOnlySessionDb((openCodeDb) =>
            computeOpenCodeWorkMetricsIncremental(openCodeDb, sessionId, carry),
        );
        workMetricsCarryBySession.set(sessionId, nextCarry);
        return metrics;
    } catch {
        return { newWorkTokens: 0, totalInputTokens: 0 };
    }
}

/** Resolves to `undefined` only when no module client exists; transport failures and daemon error responses throw, so callers cannot mistake an unreachable daemon for a session with no state. commentlint: allow(JUDGE) */
async function loadRustSessionStatus(
    client: RustModeModuleClient | undefined,
    sessionId: string,
    directory: string,
): Promise<RustSessionStatus | undefined> {
    if (!client) return undefined;
    const cacheKey = pollCacheKey(sessionId, directory);
    const cached = rustStatusCache.get(cacheKey);
    if (cached !== undefined) {
        return cached;
    }
    const response = await client.call({
        sessionId,
        projectRoot: directory,
        method: "session.status",
        body: { method: "session.status", v: 1, session_id: sessionId },
    });
    const raw =
        response && typeof response === "object" ? (response as Record<string, unknown>) : {};
    const value =
        raw.result && typeof raw.result === "object"
            ? (raw.result as Record<string, unknown>)
            : raw;
    if (value.error || value.ok === false) {
        const detail =
            value.error && typeof value.error === "object"
                ? (value.error as Record<string, unknown>)
                : undefined;
        throw new Error(
            `session.status returned ${String(detail?.code ?? detail?.message ?? value.error ?? "ok=false")}`,
        );
    }
    const status = value as RustSessionStatus;
    rustStatusCache.set(cacheKey, status);
    return status;
}

function resolveConfiguredCacheTtl(
    config: Record<string, unknown> | undefined,
    modelKey: string | undefined,
): string {
    const cacheTtlConfig = config?.cache_ttl;
    return typeof cacheTtlConfig === "string" ||
        (cacheTtlConfig !== null && typeof cacheTtlConfig === "object")
        ? resolveCacheTtl(cacheTtlConfig as EidnaraConfig["cache_ttl"], modelKey)
        : "5m";
}

function resolveToastDurationMs(config: Record<string, unknown>): number {
    const value = config.toast_duration_ms;
    return typeof value === "number" && Number.isFinite(value) ? value : 5000;
}

interface ActiveModel {
    providerID: string;
    modelID: string;
}

function parseModelKey(modelKey: string | undefined): ActiveModel | undefined {
    const slash = modelKey?.indexOf("/") ?? -1;
    if (!modelKey || slash <= 0 || slash === modelKey.length - 1) return undefined;
    return { providerID: modelKey.slice(0, slash), modelID: modelKey.slice(slash + 1) };
}

function modelKeyOf(model: ActiveModel | undefined): string | undefined {
    return model ? `${model.providerID}/${model.modelID}` : undefined;
}

/**
 * A model named by the request wins over live state. The live lookup still runs so a missing model or
 * agent is recovered from OpenCode's SQLite database and cached for later polls and hooks.
 */
function resolveActiveModel(
    sessionId: string,
    liveSessionState: LiveSessionState | undefined,
    requestedModelKey: string | undefined,
): ActiveModel | undefined {
    let liveModel: ActiveModel | undefined;
    if (liveSessionState) {
        let model = liveSessionState.liveModelBySession.get(sessionId);
        let agent = liveSessionState.agentBySession.get(sessionId);
        if (!model || !agent) {
            const recovered = findLastAssistantModelFromOpenCodeDb(sessionId);
            if (recovered) {
                if (!model) {
                    model = { providerID: recovered.providerID, modelID: recovered.modelID };
                    liveSessionState.liveModelBySession.set(sessionId, model);
                }
                if (!agent && recovered.agent) {
                    agent = recovered.agent;
                    liveSessionState.agentBySession.set(sessionId, agent);
                }
            }
        }
        liveModel = model;
    }
    return parseModelKey(requestedModelKey) ?? liveModel;
}

export function buildSidebarSnapshot(
    sessionId: string,
    directory: string,
    liveSessionState?: LiveSessionState,
    memory?: KernelMemorySnapshot,
    // The optional execute-threshold config lets the sidebar display the effective threshold with usagePercentage.
    // If the execute-threshold config is omitted, the snapshot uses the 65% runtime default.
    config?: Record<string, unknown>,
    moduleStatus?: RustSessionStatus,
    compactionEnabled = true,
    requestedModelKey?: string,
): SidebarSnapshot {
    try {
        const projectIdentity = resolveProjectIdentity(directory);

        const moduleUsage = moduleStatus?.usage;
        const moduleInputTokens = moduleUsage?.current_total_input_tokens;
        const moduleContextLimit = moduleUsage?.context_limit_tokens;
        // The daemon's usage wins; the live event usage covers `ts` mode and a daemon that has not persisted usage yet.
        const liveUsage = liveSessionState?.contextUsageBySession.get(sessionId)?.usage;
        const effectiveInputTokens =
            typeof moduleInputTokens === "number" && moduleInputTokens > 0
                ? moduleInputTokens
                : liveUsage && liveUsage.inputTokens > 0
                  ? liveUsage.inputTokens
                  : 0;
        // The sidebar computes work metrics lazily and incrementally to keep computation off the transform hot path.
        const { newWorkTokens, totalInputTokens } = resolveSidebarWorkMetrics(sessionId);

        const compartmentCount =
            typeof moduleStatus?.compartment_count === "number"
                ? moduleStatus.compartment_count
                : 0;
        const compartmentTokensLocal =
            typeof moduleStatus?.compartment_tokens === "number"
                ? moduleStatus.compartment_tokens
                : 0;
        const pendingOpsCount =
            typeof moduleStatus?.pending_drop_count === "number"
                ? moduleStatus.pending_drop_count
                : 0;
        // `wrapup_active` is the daemon's only in-flight signal, so `historianRunning` and `compartmentInProgress` share it. commentlint: allow(JUDGE)
        const wrapupActive = moduleStatus?.wrapup_active === true;

        // Expired anti-memories stay out of the count, matching the surface filter list and search apply.
        const memoryNowMs = Date.now();
        const memoryCount = memory
            ? memory.rows.filter((row) => isServedMemoryDecisionRow(row, memoryNowMs)).length
            : 0;
        const memoryTruncated = memory?.truncated === true;
        const memoryState = memory ? stateKey(memory.state) : null;

        const activeModel = resolveActiveModel(sessionId, liveSessionState, requestedModelKey);
        const activeProviderID = activeModel?.providerID;
        const activeModelID = activeModel?.modelID;
        const modelKey = modelKeyOf(activeModel);

        const contextLimit =
            typeof moduleContextLimit === "number" && moduleContextLimit > 0
                ? moduleContextLimit
                : activeProviderID && activeModelID
                  ? resolveContextLimit(activeProviderID, activeModelID)
                  : 0;
        // Usage divides by the same limit the snapshot reports, so a daemon that sent tokens without a limit still yields a percentage once the model supplies one.
        const effectiveUsagePercentage =
            contextLimit > 0 ? (effectiveInputTokens / contextLimit) * 100 : 0;

        // The sidebar uses the configured default threshold when no live model is known.
        let executeThreshold = 65;
        let executeThresholdClamped = false;
        if (config) {
            const pctCfg = config.execute_threshold_percentage as
                | number
                | { default: number; [k: string]: number }
                | undefined;
            const tokensCfg = config.execute_threshold_tokens as
                | { default?: number; [k: string]: number | undefined }
                | undefined;
            const thresholdDetail = resolveExecuteThresholdDetail(pctCfg ?? 65, modelKey, 65, {
                tokensConfig: tokensCfg,
                contextLimit: contextLimit || undefined,
                sessionId,
            });
            executeThreshold = thresholdDetail.percentage;
            executeThresholdClamped = thresholdDetail.clamped === true;
        }

        const cacheTtl = resolveConfiguredCacheTtl(config, modelKey);

        // Native compaction uses the model's unreserved context window. The daemon reports the reserved limit, so native usage remains unset without a resolved model.
        const nativeContextLimit =
            activeProviderID && activeModelID
                ? resolveContextLimit(activeProviderID, activeModelID, { reservation: "none" })
                : 0;
        const nativeContextUsagePercentage =
            nativeContextLimit > 0 ? (effectiveInputTokens / nativeContextLimit) * 100 : undefined;

        const calibration = resolveModelCalibration(activeProviderID, activeModelID);
        const tailHygiene = resolveTailHygieneStatus(moduleStatus?.tail_hygiene);
        const lastRejectError = moduleStatus?.pass_trace?.last_reject_error;
        const lastTransformError =
            typeof lastRejectError === "string" && lastRejectError !== "" ? lastRejectError : null;

        // Display-layer attribution.
        //
        // tokenizer-calibration.ts captures empirically measured per-model tokenizer drift.
        // Compartments carry the daemon's measured count; every other local bucket is zero, so the
        // conversation bucket absorbs the remainder and the buckets sum to exactly inputTokens.
        const calibrated = calibrateBuckets({
            inputTokens: effectiveInputTokens,
            systemLocal: 0,
            toolDefsLocal: 0,
            compartmentsLocal: compartmentTokensLocal,
            factsLocal: 0,
            memoriesLocal: 0,
            docsLocal: 0,
            profileLocal: 0,
            conversationLocal: 0,
            toolCallsLocal: 0,
            calibration,
        });

        const fresh: SidebarSnapshot = {
            sessionId,
            usagePercentage: effectiveUsagePercentage,
            inputTokens: effectiveInputTokens,
            contextLimit,
            native_context_usage_percentage: nativeContextUsagePercentage,
            compaction_enabled: compactionEnabled,
            systemPromptTokens: calibrated.systemTokens,
            compartmentCount,
            memoryCount,
            ...(memoryTruncated ? { memoryTruncated } : {}),
            memoryState,
            memoryBlockCount: 0,
            pendingOpsCount,
            historianRunning: wrapupActive,
            compartmentInProgress: wrapupActive,
            sessionNoteCount: 0,
            readySmartNoteCount: 0,
            cacheTtl,
            lastTransformError,
            lastDreamerRunAt: null,
            projectIdentity,
            compartmentTokens: calibrated.compartmentTokens,
            factTokens: calibrated.factTokens,
            memoryTokens: calibrated.memoryTokens,
            docsTokens: calibrated.docsTokens,
            profileTokens: calibrated.profileTokens,
            conversationTokens: calibrated.conversationTokens,
            toolCallTokens: calibrated.toolCallTokens,
            toolDefinitionTokens: calibrated.toolDefinitionTokens,
            ...(tailHygiene === undefined ? {} : { tailHygiene }),
            executeThreshold,
            executeThresholdClamped,
            boundaryPresent: moduleStatus?.boundary_present,
            coverageOrdinal: moduleStatus?.coverage_ordinal,
            newWorkTokens,
            totalInputTokens,
            recompProgress: null,
        };
        // The breakdown retains its last nonzero value when inputTokens is 0 to prevent bar flicker.
        return applyStickySnapshotCache({ sessionId, directory, modelKey }, fresh);
    } catch (err) {
        log("[rpc] sidebar-snapshot error:", err);
        throw err;
    }
}

/** Snapshot-build failures return a transport-failure envelope.
 * zero snapshot remains a successful value so deleted sessions stay deleted. */
export function buildSidebarSnapshotRpcResponse(
    sessionId: string,
    directory: string,
    liveSessionState?: LiveSessionState,
    memory?: KernelMemorySnapshot,
    config?: Record<string, unknown>,
    moduleStatus?: RustSessionStatus,
    compactionEnabled = true,
): Record<string, unknown> {
    try {
        // SAFETY: RPC results serialize to JSON; the handler map's value type is the JSON-object envelope.
        return buildSidebarSnapshot(
            sessionId,
            directory,
            liveSessionState,
            memory,
            config,
            moduleStatus,
            compactionEnabled,
        ) as unknown as Record<string, unknown>;
    } catch {
        return { error: "sidebar snapshot unavailable" };
    }
}

export function buildStatusDetail(
    sessionId: string,
    directory: string,
    modelKey?: string,
    config?: Record<string, unknown>,
    liveSessionState?: LiveSessionState,
    memory?: KernelMemorySnapshot,
    moduleStatus?: RustSessionStatus,
    compactionEnabled = true,
): StatusDetail {
    const base = buildSidebarSnapshot(
        sessionId,
        directory,
        liveSessionState,
        memory,
        config,
        moduleStatus,
        compactionEnabled,
        modelKey,
    );
    const activeModel = resolveActiveModel(sessionId, liveSessionState, modelKey);
    const effectiveModelKey = modelKeyOf(activeModel);
    // The daemon counts every minted tag and publishes no per-tag state, so only the total is known here.
    const totalTags = typeof moduleStatus?.tag_count === "number" ? moduleStatus.tag_count : 0;
    const lastResponseTime =
        liveSessionState?.contextUsageBySession.get(sessionId)?.lastResponseTime ?? 0;
    const detail: StatusDetail = {
        ...base,
        tagCounter: 0,
        activeTags: 0,
        droppedTags: 0,
        totalTags,
        activeBytes: 0,
        lastResponseTime,
        lastNudgeTokens: 0,
        isSubagent: liveSessionState?.subagentSessions.has(sessionId) ?? false,
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
        loggerDiagnostics: getLoggerDiagnostics(),
    };

    try {
        if (activeModel) {
            detail.windowGeometry = resolveContextWindowGeometry(
                activeModel.providerID,
                activeModel.modelID,
            );
        }

        const contextLimitForTokens =
            base.contextLimit > 0
                ? base.contextLimit
                : base.usagePercentage > 0
                  ? Math.round(base.inputTokens / (base.usagePercentage / 100))
                  : 0;

        if (config) {
            const pctCfg = config.execute_threshold_percentage as
                | number
                | { default: number; [k: string]: number }
                | undefined;
            const tokensCfg = config.execute_threshold_tokens as
                | { default?: number; [k: string]: number | undefined }
                | undefined;
            // The RPC uses resolveExecuteThresholdDetail to return the threshold mode and absolute-token threshold.
            const thresholdDetail = resolveExecuteThresholdDetail(
                pctCfg ?? 65,
                effectiveModelKey,
                65,
                {
                    tokensConfig: tokensCfg,
                    contextLimit: contextLimitForTokens || undefined,
                    sessionId,
                },
            );
            detail.executeThreshold = thresholdDetail.percentage;
            detail.executeThresholdMode = thresholdDetail.mode;
            detail.executeThresholdClamped = thresholdDetail.clamped;
            if (thresholdDetail.absoluteTokens !== undefined) {
                detail.executeThresholdTokens = thresholdDetail.absoluteTokens;
            }

            detail.cacheTtl = resolveConfiguredCacheTtl(config, effectiveModelKey);

            if (typeof config.protected_tags === "number") {
                detail.protectedTagCount = config.protected_tags;
            }
            if (typeof config.history_budget_percentage === "number") {
                detail.historyBudgetPercentage = config.history_budget_percentage;
            }
            detail.toastDurationMs = resolveToastDurationMs(config);
        }

        // `cacheRemainingMs` is set only after a response has been seen.
        const cacheTtlMs = parseCacheTtlMs(detail.cacheTtl) ?? DEFAULT_CACHE_TTL_MS;
        if (cacheTtlMs === Number.POSITIVE_INFINITY) {
            detail.cacheTtlMs = -1;
            detail.cacheRemainingMs = -1;
            detail.cacheNeverExpires = true;
        } else {
            detail.cacheTtlMs = cacheTtlMs;
            if (lastResponseTime > 0) {
                detail.cacheRemainingMs = Math.max(0, lastResponseTime + cacheTtlMs - Date.now());
                detail.cacheExpired = detail.cacheRemainingMs === 0;
            }
        }

        // Derived values
        if (base.contextLimit > 0) {
            detail.contextLimit = base.contextLimit;
        } else if (base.usagePercentage > 0) {
            detail.contextLimit = Math.round(base.inputTokens / (base.usagePercentage / 100));
        }

        // History compression
        try {
            const histTokens = base.compartmentTokens + base.factTokens;
            detail.historyBlockTokens = histTokens;

            if (detail.contextLimit > 0) {
                const budget = Math.floor(
                    detail.contextLimit *
                        (Math.min(detail.executeThreshold, 80) / 100) *
                        detail.historyBudgetPercentage,
                );
                detail.compressionBudget = budget;
                detail.compressionUsage = `${((histTokens / budget) * 100).toFixed(0)}%`;
            }
        } catch {}
    } catch (err) {
        log("[rpc] status-detail error:", err);
    }

    return detail;
}

export function registerRpcHandlers(
    rpcServer: EidnaraRpcServer,
    args: {
        directory: string;
        config: EidnaraConfig;
        client: unknown;
        liveSessionState: LiveSessionState;
        rustModeModuleClient?: RustModeModuleClient;
    },
): void {
    const { directory, config, liveSessionState, rustModeModuleClient } = args;
    const compactionEnabled = isCompactionEnabled(config);

    // RPC results serialize to JSON, so handler-map values use the JSON-object envelope.
    const rawConfig = config as unknown as Record<string, unknown>;

    // The status surface reports what an explicit search would see, lag included,
    // so the sidebar shows `stale` when the projector is behind.
    const kernelClient = kernelClientResolver(config);
    // The cache reuses snapshots for `RUST_STATUS_CACHE_TTL_MS` to avoid a daemon read on each sidebar poll. commentlint: allow(JUDGE)
    const memorySnapshotCache = new BoundedTtlCache<KernelMemorySnapshot>(
        RUST_STATUS_CACHE_TTL_MS,
        POLL_CACHE_MAX_ENTRIES,
    );
    const readMemory = async (sessionId: string, dir: string): Promise<KernelMemorySnapshot> => {
        if (config.memory?.enabled === false) {
            return { state: disabled(), rows: [], knownAsOf: null };
        }
        const cacheKey = pollCacheKey(sessionId, dir);
        const cached = memorySnapshotCache.get(cacheKey);
        if (cached !== undefined) {
            return cached;
        }
        const client = kernelClient({ sessionId, projectRoot: resolveProjectRootDirectory(dir) });
        const snapshot = kernelMemorySnapshotFrom(
            await client.read({ surface: "explicit_search", gated: true }),
        );
        memorySnapshotCache.set(cacheKey, snapshot);
        return snapshot;
    };

    // An unreachable daemon fails the poll rather than yielding zero counts, because `applyStickySnapshotCache` treats zero counts as lost state and blanks the sidebar. commentlint: allow(JUDGE)
    const loadPollInputs = async (
        sessionId: string,
        dir: string,
    ): Promise<{ moduleStatus?: RustSessionStatus; memory: KernelMemorySnapshot } | undefined> => {
        try {
            const [moduleStatus, memory] = await Promise.all([
                config.transform_mode === "rust"
                    ? loadRustSessionStatus(rustModeModuleClient, sessionId, dir)
                    : Promise.resolve(undefined),
                readMemory(sessionId, dir),
            ]);
            return { moduleStatus, memory };
        } catch (error) {
            log(`[rpc] session.status unavailable for ${sessionId}:`, error);
            return undefined;
        }
    };

    rpcServer.handle("sidebar-snapshot", async (params) => {
        const sessionId = String(params.sessionId ?? "");
        const dir = String(params.directory ?? directory);
        if (!sessionId) return { error: "unavailable" };
        const inputs = await loadPollInputs(sessionId, dir);
        if (!inputs) return { error: "sidebar snapshot unavailable" };
        return buildSidebarSnapshotRpcResponse(
            sessionId,
            dir,
            liveSessionState,
            inputs.memory,
            rawConfig,
            inputs.moduleStatus,
            compactionEnabled,
        );
    });

    rpcServer.handle("status-detail", async (params) => {
        const sessionId = String(params.sessionId ?? "");
        const dir = String(params.directory ?? directory);
        const modelKey = params.modelKey ? String(params.modelKey) : undefined;
        if (!sessionId) return { error: "unavailable" };
        const inputs = await loadPollInputs(sessionId, dir);
        if (!inputs) return { error: "status detail unavailable" };
        return buildStatusDetail(
            sessionId,
            dir,
            modelKey,
            rawConfig,
            liveSessionState,
            inputs.memory,
            inputs.moduleStatus,
            compactionEnabled,
        ) as unknown as Record<string, unknown>;
    });

    rpcServer.handle("toast-duration", async () => ({
        toastDurationMs: resolveToastDurationMs(rawConfig),
    }));
}
