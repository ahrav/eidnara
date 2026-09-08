/// <reference types="bun-types" />

import { afterEach, describe, expect, test } from "bun:test";
import { EidnaraConfigSchema } from "../config/schema/eidnara";
import { resetKernelClientsForTest } from "../hooks/context/kernel-transport";
import { createLiveSessionState } from "../hooks/context/live-session-state";
import type { RustModeModuleClient } from "../hooks/context/rust-mode-transform";
import { unavailable } from "../shared/kernel-client";
import { ANTI_MEMORY_CATEGORY, renderAntiMemoryContent } from "../shared/kernel-client/anti-memory";
import { FakeKernel } from "../shared/kernel-client-testing/fake-kernel";
import type { EidnaraRpcServer } from "../shared/rpc-server";
import { formatMemoryCount, type SidebarSnapshot, type StatusDetail } from "../shared/rpc-types";
import {
    BoundedTtlCache,
    buildSidebarSnapshot,
    buildSidebarSnapshotRpcResponse,
    buildStatusDetail,
    type RustSessionStatus,
    registerRpcHandlers,
} from "./rpc-handlers";
import { resetSidebarSnapshotCache } from "./sidebar-snapshot-cache";

type Handler = (params: Record<string, unknown>) => Promise<Record<string, unknown>>;

const MISSING_CONNECTION_FILE = "/nonexistent/eidnara-rpc-handlers-test/subc.json";
const DAEMON_STATUS: RustSessionStatus = {
    usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
    boundary_present: true,
    coverage_ordinal: 17,
    compartment_count: 4,
    compartment_tokens: 23,
    pending_drop_count: 2,
    wrapup_active: true,
    tail_hygiene: {
        u: 65_100,
        t: 100_000,
        severity: 0.651,
        evaluable: true,
        generation_invalidated: false,
        baseline_generation: 7,
        computed_at_ms: 123,
    },
};

/** Registers against a handler map: the fake server captures registrations without starting a transport. */
function register(
    configOverrides: Record<string, unknown> = {},
    status: RustSessionStatus = {},
): { handlers: Map<string, Handler>; calls: string[] } {
    const handlers = new Map<string, Handler>();
    const calls: string[] = [];
    const server = {
        handle(method: string, handler: Handler) {
            handlers.set(method, handler);
        },
    } as unknown as EidnaraRpcServer;
    const rustModeModuleClient: RustModeModuleClient = {
        async call(args) {
            calls.push(args.method);
            return { ok: true, result: status };
        },
    };
    registerRpcHandlers(server, {
        directory: process.cwd(),
        config: EidnaraConfigSchema.parse({
            transform_mode: "rust",
            subc: { connection_file: MISSING_CONNECTION_FILE },
            ...configOverrides,
        }),
        client: null,
        liveSessionState: createLiveSessionState(),
        rustModeModuleClient,
    });
    return { handlers, calls };
}

function seedMemories(kernel: FakeKernel): void {
    kernel.seedDecision({
        object_id: `mem_${"a".repeat(32)}`,
        decision_kind: "PROJECT_RULES",
        summary: "Always use Bun for builds",
    });
    kernel.seedDecision({
        object_id: `mem_${"b".repeat(32)}`,
        decision_kind: "ARCHITECTURE",
        summary: "OpenCode source lives at ~/Work/OSS/opencode.",
    });
}

function seedAntiMemory(kernel: FakeKernel, idChar: string, expiresAt: number): void {
    kernel.seedDecision({
        object_id: `mem_${idChar.repeat(32)}`,
        decision_kind: ANTI_MEMORY_CATEGORY,
        summary: renderAntiMemoryContent({
            trigger: "asked to bypass the daemon",
            rejectedStrategy: "write straight to the store",
            rejectionReason: "the daemon owns commit ordering",
            expiresAt,
        }),
    });
}

afterEach(() => {
    resetSidebarSnapshotCache();
    resetKernelClientsForTest();
});

describe("registerRpcHandlers", () => {
    test("registers exactly sidebar-snapshot, status-detail, and toast-duration", async () => {
        const { handlers } = register({ toast_duration_ms: 1234 });
        expect([...handlers.keys()].sort()).toEqual([
            "sidebar-snapshot",
            "status-detail",
            "toast-duration",
        ]);
        expect(await handlers.get("toast-duration")?.({})).toEqual({ toastDurationMs: 1234 });
    });

    test("sidebar-snapshot maps the daemon status and status-detail reuses the cached daemon answer", async () => {
        const { handlers, calls } = register({}, DAEMON_STATUS);
        const sessionId = "ses-handler-sidebar";
        const snapshot = (await handlers.get("sidebar-snapshot")?.({
            sessionId,
        })) as unknown as SidebarSnapshot;

        expect(calls).toEqual(["session.status"]);
        expect(snapshot.inputTokens).toBe(42_000);
        expect(snapshot.usagePercentage).toBe(42);
        expect(snapshot.contextLimit).toBe(100_000);
        expect(snapshot.compartmentCount).toBe(4);
        expect(snapshot.pendingOpsCount).toBe(2);
        expect(snapshot.memoryCount).toBe(0);
        expect(snapshot.memoryState).toBe("unavailable:daemon_absent");

        const detail = (await handlers.get("status-detail")?.({
            sessionId,
        })) as unknown as StatusDetail;
        expect(calls).toEqual(["session.status"]);
        expect(detail.compartmentCount).toBe(4);
    });

    test("sidebar-snapshot reports disabled memory and rejects an empty session id", async () => {
        const { handlers } = register({ memory: { enabled: false } });
        const snapshot = (await handlers.get("sidebar-snapshot")?.({
            sessionId: "ses-handler-disabled",
        })) as unknown as SidebarSnapshot;
        expect(snapshot.memoryState).toBe("disabled");
        expect(await handlers.get("sidebar-snapshot")?.({})).toEqual({ error: "unavailable" });
    });
});

describe("buildSidebarSnapshot — daemon status", () => {
    test("maps pressure, boundary, coverage, compartments, wrap-up, tail hygiene, and neutral fields", () => {
        const snapshot = buildSidebarSnapshot(
            "ses-sidebar-rust-status",
            process.cwd(),
            createLiveSessionState(),
            undefined,
            undefined,
            DAEMON_STATUS,
        );

        expect(snapshot.inputTokens).toBe(42_000);
        expect(snapshot.usagePercentage).toBe(42);
        expect(snapshot.contextLimit).toBe(100_000);
        expect(snapshot.compartmentCount).toBe(4);
        expect(snapshot.compartmentTokens).toBe(23);
        expect(snapshot.pendingOpsCount).toBe(2);
        expect(snapshot.boundaryPresent).toBe(true);
        expect(snapshot.coverageOrdinal).toBe(17);
        expect(snapshot.historianRunning).toBe(true);
        expect(snapshot.compartmentInProgress).toBe(true);
        expect(snapshot.tailHygiene).toEqual({
            u: 65_100,
            t: 100_000,
            severity: 0.651,
            evaluable: true,
            generationInvalidated: false,
            baselineGeneration: 7,
            computedAt: 123,
        });

        // Every token bucket sums to inputTokens; the daemon's compartment count is the only nonzero local bucket.
        const sum =
            snapshot.systemPromptTokens +
            snapshot.toolDefinitionTokens +
            snapshot.compartmentTokens +
            snapshot.factTokens +
            snapshot.memoryTokens +
            snapshot.docsTokens +
            snapshot.profileTokens +
            snapshot.conversationTokens +
            snapshot.toolCallTokens;
        expect(sum).toBe(42_000);

        expect(snapshot.memoryBlockCount).toBe(0);
        expect(snapshot.sessionNoteCount).toBe(0);
        expect(snapshot.readySmartNoteCount).toBe(0);
        expect(snapshot.lastTransformError).toBeNull();
        expect(snapshot.lastDreamerRunAt).toBeNull();
        expect(snapshot.factTokens).toBe(0);
        expect(snapshot.memoryTokens).toBe(0);
        expect(snapshot.recompProgress).toBeNull();
        expect(Object.hasOwn(snapshot as object, "archivedCompartmentCount")).toBe(false);
    });

    test("an absent daemon yields a successful zero snapshot with idle progress flags", () => {
        const snapshot = buildSidebarSnapshot("ses-sidebar-no-daemon", process.cwd());
        expect(snapshot.inputTokens).toBe(0);
        expect(snapshot.usagePercentage).toBe(0);
        expect(snapshot.contextLimit).toBe(0);
        expect(snapshot.compartmentCount).toBe(0);
        expect(snapshot.pendingOpsCount).toBe(0);
        expect(snapshot.historianRunning).toBe(false);
        expect(snapshot.compartmentInProgress).toBe(false);
        expect(snapshot.tailHygiene).toBeUndefined();
        const response = buildSidebarSnapshotRpcResponse("ses-empty", process.cwd());
        expect(response).toMatchObject({ sessionId: "ses-empty", inputTokens: 0 });
    });

    test("reports the resolved compaction mode and raw native usage", () => {
        const snapshot = buildSidebarSnapshot(
            "ses-native-sidebar",
            process.cwd(),
            undefined,
            undefined,
            { execute_threshold_percentage: 65 },
            { usage: { current_total_input_tokens: 41_000, context_limit_tokens: 100_000 } },
            false,
        );
        expect(snapshot.compaction_enabled).toBe(false);
        expect(snapshot.native_context_usage_percentage).toBe(41);
        expect(snapshot.cacheTtl).toBe("5m");
    });

    test("falls back to the live model's context limit and per-model config when the daemon gives none", () => {
        const sessionId = "ses-sidebar-live-limit";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        const snapshot = buildSidebarSnapshot(sessionId, process.cwd(), live, undefined, {
            execute_threshold_percentage: { default: 65, "test-provider/test-model": 50 },
            cache_ttl: { default: "5m", "test-provider/test-model": "10m" },
        });
        expect(snapshot.contextLimit).toBe(128_000);
        expect(snapshot.native_context_usage_percentage).toBe(0);
        expect(snapshot.executeThreshold).toBe(50);
        expect(snapshot.cacheTtl).toBe("10m");
    });
});

describe("buildSidebarSnapshot — kernel memory", () => {
    test("reports the kernel's row count, state, and truncation", () => {
        const kernel = new FakeKernel();
        seedMemories(kernel);
        // The sidebar count excludes decisions outside the memory domain.
        kernel.seedDecision({
            object_id: `mem_${"c".repeat(32)}`,
            decision_kind: "ARCHITECTURE",
            summary: "A foreign-domain decision.",
            domain_id: "notes",
        });

        const snapshot = buildSidebarSnapshot(
            "ses-memory",
            process.cwd(),
            undefined,
            kernel.snapshot("explicit_search"),
        );
        expect(snapshot.memoryCount).toBe(2);
        expect(snapshot.memoryState).toBe("available");
        expect(snapshot.memoryTruncated).toBeUndefined();
        expect(formatMemoryCount(snapshot)).toBe("2");

        kernel.readTruncated = true;
        const truncated = buildSidebarSnapshot(
            "ses-truncated",
            process.cwd(),
            undefined,
            kernel.snapshot("explicit_search"),
        );
        expect(truncated.memoryTruncated).toBe(true);
        expect(formatMemoryCount(truncated)).toBe("2+");

        const absent = buildSidebarSnapshot("ses-memory-absent", process.cwd(), undefined, {
            state: unavailable("daemon_absent"),
            rows: [],
            knownAsOf: null,
        });
        expect(absent.memoryCount).toBe(0);
        expect(absent.memoryState).toBe("unavailable:daemon_absent");
    });

    test("an expired anti-memory stays out of the memory count", () => {
        const kernel = new FakeKernel();
        seedMemories(kernel);
        seedAntiMemory(kernel, "c", 1);
        seedAntiMemory(kernel, "d", Date.now() + 60_000);

        const snapshot = buildSidebarSnapshot(
            "ses-expired-anti",
            process.cwd(),
            undefined,
            kernel.snapshot("explicit_search"),
        );
        expect(snapshot.memoryCount).toBe(3);
    });
});

describe("buildStatusDetail", () => {
    test("derives dialog fields from the live model, the daemon, and config; storage-only fields stay neutral", () => {
        const sessionId = "ses-status-detail";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        live.subagentSessions.add(sessionId);
        const detail = buildStatusDetail(
            sessionId,
            process.cwd(),
            "test-provider/test-model",
            { execute_threshold_percentage: 65, toast_duration_ms: 2500, cache_ttl: "never" },
            live,
            undefined,
            DAEMON_STATUS,
        );

        expect(detail.contextLimit).toBe(100_000);
        expect(detail.compaction_enabled).toBe(true);
        expect(detail.isSubagent).toBe(true);
        expect(detail.toastDurationMs).toBe(2500);
        expect(detail.cacheTtl).toBe("never");
        expect(detail.executeThreshold).toBe(65);
        expect(detail.executeThresholdMode).toBe("percentage");
        expect(detail.historyBlockTokens).toBe(detail.compartmentTokens);
        expect(detail.compressionBudget).toBe(Math.floor(100_000 * 0.65 * 0.15));
        expect(detail.loggerDiagnostics).toEqual({
            swallowedWriteCount: 0,
            lastErrorMessage: null,
            lastErrorTime: null,
        });

        const bare = buildStatusDetail("ses-status-neutral", process.cwd());
        expect(bare.tagCounter).toBe(0);
        expect(bare.activeTags).toBe(0);
        expect(bare.pendingOps).toEqual([]);
        expect(bare.isSubagent).toBe(false);
        expect(bare.lastResponseTime).toBe(0);
        expect(bare.cacheTtlMs).toBe(0);
        expect(bare.cacheRemainingMs).toBe(0);
        expect(bare.cacheExpired).toBe(false);
        expect(bare.cacheNeverExpires).toBe(false);
    });
});

describe("BoundedTtlCache", () => {
    test("get drops an expired entry and set sweeps every expired entry", () => {
        const cache = new BoundedTtlCache<string>(2_000, 32);
        cache.set("a", "alpha", 1_000);
        expect(cache.get("a", 2_500)).toBe("alpha");
        expect(cache.get("a", 3_000)).toBeUndefined();
        expect(cache.size).toBe(0);
        cache.set("a", "alpha", 1_000);
        cache.set("b", "beta", 1_000);
        cache.set("c", "gamma", 5_000);
        expect(cache.size).toBe(1);
        expect(cache.get("c", 5_000)).toBe("gamma");
    });

    test("a full cache evicts its oldest entry on insert but refreshes a live key in place", () => {
        const cache = new BoundedTtlCache<string>(60_000, 2);
        cache.set("a", "alpha", 1_000);
        cache.set("b", "beta", 2_000);
        cache.set("b", "beta-2", 2_500);
        expect(cache.size).toBe(2);
        expect(cache.get("a", 2_500)).toBe("alpha");
        expect(cache.get("b", 2_500)).toBe("beta-2");
        cache.set("c", "gamma", 3_000);
        expect(cache.size).toBe(2);
        expect(cache.get("a", 3_000)).toBeUndefined();
        expect(cache.get("b", 3_000)).toBe("beta-2");
        expect(cache.get("c", 3_000)).toBe("gamma");
    });

    test("an infinite TTL never expires an entry but the size cap still evicts the oldest", () => {
        const cache = new BoundedTtlCache<string>(Number.POSITIVE_INFINITY, 2);
        cache.set("a", "alpha", 1_000);
        cache.set("b", "beta", 2_000);
        expect(cache.get("a", Number.MAX_SAFE_INTEGER)).toBe("alpha");
        expect(cache.size).toBe(2);
        cache.set("c", "gamma", 3_000);
        expect(cache.size).toBe(2);
        expect(cache.get("a", 4_000)).toBeUndefined();
        expect(cache.get("b", 4_000)).toBe("beta");
        expect(cache.get("c", 4_000)).toBe("gamma");
    });
});
