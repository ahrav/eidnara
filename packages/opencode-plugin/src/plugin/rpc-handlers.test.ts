/// <reference types="bun-types" />

import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { EidnaraConfigSchema } from "../config/schema/eidnara";
import { createEventHandler } from "../hooks/context/event-handler";
import { resetKernelClientsForTest } from "../hooks/context/kernel-transport";
import { createLiveSessionState } from "../hooks/context/live-session-state";
import { closeReadOnlySessionDb } from "../hooks/context/read-session-db";
import type { RustModeModuleClient } from "../hooks/context/rust-mode-transform";
import { unavailable } from "../shared/kernel-client";
import { ANTI_MEMORY_CATEGORY, renderAntiMemoryContent } from "../shared/kernel-client/anti-memory";
import { FakeKernel } from "../shared/kernel-client-testing/fake-kernel";
import type { EidnaraRpcServer } from "../shared/rpc-server";
import { formatMemoryCount, type SidebarSnapshot, type StatusDetail } from "../shared/rpc-types";
import { Database } from "../shared/sqlite";
import { closeQuietly } from "../shared/sqlite-helpers";
import {
    BoundedTtlCache,
    buildSidebarSnapshot,
    buildSidebarSnapshotRpcResponse,
    buildStatusDetail,
    clearRustSessionStatus,
    clearWorkMetricsCarry,
    clearWorkMetricsCarryIfFolded,
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
    status: RustSessionStatus | Error = {},
    liveSessionState = createLiveSessionState(),
): { handlers: Map<string, Handler>; calls: string[]; roots: string[] } {
    const handlers = new Map<string, Handler>();
    const calls: string[] = [];
    const roots: string[] = [];
    const server = {
        handle(method: string, handler: Handler) {
            handlers.set(method, handler);
        },
    } as unknown as EidnaraRpcServer;
    const rustModeModuleClient: RustModeModuleClient = {
        async call(args) {
            calls.push(args.method);
            roots.push(args.projectRoot);
            if (status instanceof Error) throw status;
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
        liveSessionState,
        rustModeModuleClient,
    });
    return { handlers, calls, roots };
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

    test("the daemon status cache keys by project root, so one session polled under two roots asks each route", async () => {
        const { handlers, calls, roots } = register({}, DAEMON_STATUS);
        const sessionId = "ses-handler-two-roots";
        const rootA = process.cwd();
        const rootB = join(process.cwd(), "src");

        await handlers.get("sidebar-snapshot")?.({ sessionId, directory: rootA });
        await handlers.get("sidebar-snapshot")?.({ sessionId, directory: rootB });
        await handlers.get("sidebar-snapshot")?.({ sessionId, directory: rootA });

        expect(calls).toEqual(["session.status", "session.status"]);
        expect(roots).toEqual([rootA, rootB]);
    });

    test("a zero-token answer from a second root does not inherit the first root's sticky totals", async () => {
        const sessionId = "ses-handler-sticky-roots";
        const rootA = process.cwd();
        const rootB = join(process.cwd(), "src");
        // Root B keeps root A's compartment count so a session-keyed sticky cache would substitute A's totals.
        const statusByRoot = new Map<string, RustSessionStatus>([
            [rootA, DAEMON_STATUS],
            [
                rootB,
                {
                    usage: { current_total_input_tokens: 0, context_limit_tokens: 100_000 },
                    compartment_count: DAEMON_STATUS.compartment_count,
                },
            ],
        ]);
        const handlers = new Map<string, Handler>();
        const server = {
            handle(method: string, handler: Handler) {
                handlers.set(method, handler);
            },
        } as unknown as EidnaraRpcServer;
        registerRpcHandlers(server, {
            directory: rootA,
            config: EidnaraConfigSchema.parse({
                transform_mode: "rust",
                subc: { connection_file: MISSING_CONNECTION_FILE },
            }),
            client: null,
            liveSessionState: createLiveSessionState(),
            rustModeModuleClient: {
                async call(args) {
                    return { ok: true, result: statusByRoot.get(args.projectRoot) ?? {} };
                },
            },
        });

        const first = (await handlers.get("sidebar-snapshot")?.({
            sessionId,
            directory: rootA,
        })) as unknown as SidebarSnapshot;
        expect(first.inputTokens).toBe(42_000);

        const second = (await handlers.get("sidebar-snapshot")?.({
            sessionId,
            directory: rootB,
        })) as unknown as SidebarSnapshot;
        expect(second.inputTokens).toBe(0);
        expect(second.usagePercentage).toBe(0);
    });

    test("a daemon that cannot answer fails the poll instead of returning a zero snapshot", async () => {
        const { handlers, calls } = register({}, new Error("route closed"));
        const sessionId = "ses-handler-daemon-down";

        expect(await handlers.get("sidebar-snapshot")?.({ sessionId })).toEqual({
            error: "sidebar snapshot unavailable",
        });
        expect(await handlers.get("status-detail")?.({ sessionId })).toEqual({
            error: "status detail unavailable",
        });
        // A failure is not cached, so the next poll asks the daemon again.
        expect(calls).toEqual(["session.status", "session.status"]);
    });

    test("a daemon error response fails the poll the same way", async () => {
        const handlers = new Map<string, Handler>();
        const server = {
            handle(method: string, handler: Handler) {
                handlers.set(method, handler);
            },
        } as unknown as EidnaraRpcServer;
        registerRpcHandlers(server, {
            directory: process.cwd(),
            config: EidnaraConfigSchema.parse({
                transform_mode: "rust",
                subc: { connection_file: MISSING_CONNECTION_FILE },
            }),
            client: null,
            liveSessionState: createLiveSessionState(),
            rustModeModuleClient: {
                async call() {
                    return { ok: false, error: { code: "store_load_failed", message: "io" } };
                },
            },
        });
        expect(await handlers.get("sidebar-snapshot")?.({ sessionId: "ses-daemon-error" })).toEqual(
            { error: "sidebar snapshot unavailable" },
        );
    });

    test("concurrent polls that miss the status cache share one daemon request", async () => {
        const sessionId = "ses-handler-inflight";
        let calls = 0;
        let release: (() => void) | undefined;
        const gate = new Promise<void>((resolve) => {
            release = resolve;
        });
        const handlers = new Map<string, Handler>();
        const server = {
            handle(method: string, handler: Handler) {
                handlers.set(method, handler);
            },
        } as unknown as EidnaraRpcServer;
        registerRpcHandlers(server, {
            directory: process.cwd(),
            config: EidnaraConfigSchema.parse({
                transform_mode: "rust",
                subc: { connection_file: MISSING_CONNECTION_FILE },
            }),
            client: null,
            liveSessionState: createLiveSessionState(),
            rustModeModuleClient: {
                async call() {
                    calls += 1;
                    await gate;
                    return { ok: true, result: DAEMON_STATUS };
                },
            },
        });

        const first = handlers.get("sidebar-snapshot")?.({ sessionId });
        const second = handlers.get("status-detail")?.({ sessionId });
        release?.();
        const [snapshot, detail] = (await Promise.all([first, second])) as unknown as [
            SidebarSnapshot,
            StatusDetail,
        ];
        expect(calls).toBe(1);
        expect(snapshot.compartmentCount).toBe(4);
        expect(detail.compartmentCount).toBe(4);
    });

    test("clearRustSessionStatus forgets the cached status and fences a request in flight", async () => {
        const sessionId = "ses-handler-status-cleared";
        let calls = 0;
        let release: (() => void) | undefined;
        const gate = new Promise<void>((resolve) => {
            release = resolve;
        });
        const handlers = new Map<string, Handler>();
        const server = {
            handle(method: string, handler: Handler) {
                handlers.set(method, handler);
            },
        } as unknown as EidnaraRpcServer;
        registerRpcHandlers(server, {
            directory: process.cwd(),
            config: EidnaraConfigSchema.parse({
                transform_mode: "rust",
                subc: { connection_file: MISSING_CONNECTION_FILE },
            }),
            client: null,
            liveSessionState: createLiveSessionState(),
            rustModeModuleClient: {
                async call() {
                    calls += 1;
                    if (calls === 3) await gate;
                    return { ok: true, result: DAEMON_STATUS };
                },
            },
        });

        await handlers.get("sidebar-snapshot")?.({ sessionId });
        expect(calls).toBe(1);
        // Within the TTL a poll would reuse the cache; the clear forces a fresh daemon read.
        clearRustSessionStatus(sessionId);
        await handlers.get("sidebar-snapshot")?.({ sessionId });
        expect(calls).toBe(2);

        // A clear while a request is in flight keeps its late answer out of the cache.
        clearRustSessionStatus(sessionId);
        const pending = handlers.get("sidebar-snapshot")?.({ sessionId });
        expect(calls).toBe(3);
        clearRustSessionStatus(sessionId);
        release?.();
        await pending;
        await handlers.get("sidebar-snapshot")?.({ sessionId });
        expect(calls).toBe(4);
    });

    test("sidebar-snapshot reports disabled memory and rejects an empty session id", async () => {
        const { handlers } = register({ memory: { enabled: false } });
        const snapshot = (await handlers.get("sidebar-snapshot")?.({
            sessionId: "ses-handler-disabled",
        })) as unknown as SidebarSnapshot;
        expect(snapshot.memoryState).toBe("disabled");
        expect(await handlers.get("sidebar-snapshot")?.({})).toEqual({ error: "unavailable" });
    });

    test("ts mode serves the live event usage without contacting the daemon", async () => {
        const sessionId = "ses-handler-ts-mode";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 50, inputTokens: 64_000 },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
            model: { providerID: "test-provider", modelID: "test-model" },
        });
        const { handlers, calls } = register({ transform_mode: "ts" }, DAEMON_STATUS, live);

        const snapshot = (await handlers.get("sidebar-snapshot")?.({
            sessionId,
        })) as unknown as SidebarSnapshot;
        expect(calls).toEqual([]);
        expect(snapshot.inputTokens).toBe(64_000);
        expect(snapshot.usagePercentage).toBe(50);
        expect(snapshot.compartmentCount).toBe(0);

        const detail = (await handlers.get("status-detail")?.({
            sessionId,
        })) as unknown as StatusDetail;
        expect(calls).toEqual([]);
        expect(detail.inputTokens).toBe(64_000);
        expect(detail.lastResponseTime).toBeGreaterThan(0);
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

    test("reports the resolved compaction mode and leaves native usage unset without a model", () => {
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
        expect(snapshot.native_context_usage_percentage).toBeUndefined();
        expect(snapshot.usagePercentage).toBe(41);
        expect(snapshot.cacheTtl).toBe("5m");
    });

    test("native usage divides by the live model's unreserved window", () => {
        const sessionId = "ses-native-live";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        const snapshot = buildSidebarSnapshot(
            sessionId,
            process.cwd(),
            live,
            undefined,
            undefined,
            {
                usage: { current_total_input_tokens: 64_000, context_limit_tokens: 100_000 },
            },
        );
        expect(snapshot.usagePercentage).toBe(64);
        expect(snapshot.native_context_usage_percentage).toBe(50);
    });

    test("usage divides by the model's limit when the daemon sends tokens without a limit", () => {
        const sessionId = "ses-usage-fallback-limit";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        const snapshot = buildSidebarSnapshot(
            sessionId,
            process.cwd(),
            live,
            undefined,
            undefined,
            { usage: { current_total_input_tokens: 32_000 } },
        );
        expect(snapshot.inputTokens).toBe(32_000);
        expect(snapshot.contextLimit).toBe(128_000);
        expect(snapshot.usagePercentage).toBe(25);
    });

    test("without daemon usage the sidebar reports the live event usage", () => {
        const sessionId = "ses-live-usage";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 50, inputTokens: 64_000 },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
            model: { providerID: "test-provider", modelID: "test-model" },
        });
        const fromLive = buildSidebarSnapshot(sessionId, process.cwd(), live);
        expect(fromLive.inputTokens).toBe(64_000);
        expect(fromLive.contextLimit).toBe(128_000);
        expect(fromLive.usagePercentage).toBe(50);
        expect(fromLive.native_context_usage_percentage).toBe(50);

        // Daemon usage wins when present.
        const fromDaemon = buildSidebarSnapshot(
            sessionId,
            process.cwd(),
            live,
            undefined,
            undefined,
            {
                usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
            },
        );
        expect(fromDaemon.inputTokens).toBe(42_000);
        expect(fromDaemon.usagePercentage).toBe(42);
    });

    test("live usage measured against a different model is not shown after a model switch", () => {
        const sessionId = "ses-live-usage-stale-model";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, { providerID: "test-provider", modelID: "large" });
        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 50, inputTokens: 64_000 },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
            model: { providerID: "test-provider", modelID: "small" },
        });
        const snapshot = buildSidebarSnapshot(sessionId, process.cwd(), live);
        expect(snapshot.inputTokens).toBe(0);
        expect(snapshot.usagePercentage).toBe(0);

        // An entry without a model cannot be matched to the active model either.
        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 50, inputTokens: 64_000 },
            updatedAt: Date.now(),
            hasUsageTokens: true,
        });
        expect(buildSidebarSnapshot(sessionId, process.cwd(), live).inputTokens).toBe(0);
    });

    test("surfaces the daemon's last transform rejection", () => {
        const rejected = buildSidebarSnapshot(
            "ses-rejected",
            process.cwd(),
            undefined,
            undefined,
            undefined,
            {
                ...DAEMON_STATUS,
                pass_trace: { last_reject_error: "wire page exceeded budget" },
            },
        );
        expect(rejected.lastTransformError).toBe("wire page exceeded budget");
        expect(
            buildStatusDetail(
                "ses-rejected",
                process.cwd(),
                undefined,
                undefined,
                undefined,
                undefined,
                {
                    ...DAEMON_STATUS,
                    pass_trace: { last_reject_error: "wire page exceeded budget" },
                },
            ).lastTransformError,
        ).toBe("wire page exceeded budget");

        const clean = buildSidebarSnapshot(
            "ses-clean",
            process.cwd(),
            undefined,
            undefined,
            undefined,
            {
                ...DAEMON_STATUS,
                pass_trace: { last_reject_error: null },
            },
        );
        expect(clean.lastTransformError).toBeNull();
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
        expect(detail.cacheTtlMs).toBe(-1);
        expect(detail.cacheRemainingMs).toBe(-1);
        expect(detail.cacheNeverExpires).toBe(true);
        expect(detail.cacheExpired).toBe(false);
        expect(detail.totalTags).toBe(0);
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
        // The default "5m" TTL normalizes to milliseconds; no response has been seen, so nothing counts down.
        expect(bare.cacheTtl).toBe("5m");
        expect(bare.cacheTtlMs).toBe(300_000);
        expect(bare.cacheRemainingMs).toBe(0);
        expect(bare.cacheExpired).toBe(false);
        expect(bare.cacheNeverExpires).toBe(false);
    });

    test("cache countdown and tag totals follow the live response time and daemon status", () => {
        const sessionId = "ses-status-countdown";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        const now = Date.now();
        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 10, inputTokens: 10_000 },
            updatedAt: now,
            lastResponseTime: now - 60_000,
            hasUsageTokens: true,
            model: { providerID: "test-provider", modelID: "test-model" },
        });
        const detail = buildStatusDetail(
            sessionId,
            process.cwd(),
            undefined,
            { cache_ttl: "5m" },
            live,
            undefined,
            { ...DAEMON_STATUS, tag_count: 12 },
        );
        expect(detail.totalTags).toBe(12);
        expect(detail.lastResponseTime).toBe(now - 60_000);
        expect(detail.cacheTtlMs).toBe(300_000);
        expect(detail.cacheRemainingMs).toBeGreaterThan(230_000);
        expect(detail.cacheRemainingMs).toBeLessThanOrEqual(240_000);
        expect(detail.cacheExpired).toBe(false);

        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 10, inputTokens: 10_000 },
            updatedAt: now,
            lastResponseTime: now - 600_000,
            hasUsageTokens: true,
            model: { providerID: "test-provider", modelID: "test-model" },
        });
        const expired = buildStatusDetail(
            sessionId,
            process.cwd(),
            undefined,
            { cache_ttl: "5m" },
            live,
        );
        expect(expired.cacheRemainingMs).toBe(0);
        expect(expired.cacheExpired).toBe(true);

        // A response recorded for another model does not start the new model's countdown.
        const switched = buildStatusDetail(
            sessionId,
            process.cwd(),
            "test-provider/other-model",
            { cache_ttl: "5m" },
            live,
        );
        expect(switched.lastResponseTime).toBe(0);
        expect(switched.cacheRemainingMs).toBe(0);
        expect(switched.cacheExpired).toBe(false);
    });

    test("an unparseable cache TTL falls back to the daemon's five-minute default", () => {
        const sessionId = "ses-status-bad-ttl";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        const now = Date.now();
        live.contextUsageBySession.set(sessionId, {
            usage: { percentage: 10, inputTokens: 10_000 },
            updatedAt: now,
            lastResponseTime: now - 60_000,
            hasUsageTokens: true,
            model: { providerID: "test-provider", modelID: "test-model" },
        });
        for (const cacheTtl of ["5d", ""]) {
            const detail = buildStatusDetail(
                sessionId,
                process.cwd(),
                undefined,
                { cache_ttl: cacheTtl },
                live,
            );
            expect(detail.cacheTtl).toBe(cacheTtl);
            expect(detail.cacheTtlMs).toBe(300_000);
            expect(detail.cacheRemainingMs).toBeGreaterThan(230_000);
            expect(detail.cacheNeverExpires).toBe(false);
        }
    });

    test("the compression budget applies the effective threshold without an extra cap", () => {
        const detail = buildStatusDetail(
            "ses-status-budget-90",
            process.cwd(),
            undefined,
            { execute_threshold_percentage: 90 },
            undefined,
            undefined,
            DAEMON_STATUS,
        );
        expect(detail.executeThreshold).toBe(90);
        // Mirrors resolveHistoryBudgetTokens: 100k * 0.90 * 0.15.
        expect(detail.compressionBudget).toBe(13_500);
    });

    test("a request without modelKey resolves per-model config from the live model", () => {
        const sessionId = "ses-status-live-model";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model",
        });
        const config = {
            execute_threshold_percentage: { default: 65, "test-provider/test-model": 50 },
            cache_ttl: { default: "5m", "test-provider/test-model": "10m" },
            toast_duration_ms: 1500,
        };

        const fromLive = buildStatusDetail(sessionId, process.cwd(), undefined, config, live);
        expect(fromLive.executeThreshold).toBe(50);
        expect(fromLive.cacheTtl).toBe("10m");
        expect(fromLive.toastDurationMs).toBe(1500);

        // An explicit request key wins over the live model.
        const requested = buildStatusDetail(
            sessionId,
            process.cwd(),
            "other-provider/other-model",
            config,
            live,
        );
        expect(requested.executeThreshold).toBe(65);
        expect(requested.cacheTtl).toBe("5m");
    });

    test("the dialog's cache TTL matches the sidebar's base-model lookup", () => {
        const sessionId = "ses-status-derived-model";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, {
            providerID: "test-provider",
            modelID: "test-model-fast",
        });
        const config = { cache_ttl: { default: "5m", "test-provider/test-model": "10m" } };

        const snapshot = buildSidebarSnapshot(sessionId, process.cwd(), live, undefined, config);
        const detail = buildStatusDetail(sessionId, process.cwd(), undefined, config, live);
        expect(snapshot.cacheTtl).toBe("10m");
        expect(detail.cacheTtl).toBe("10m");
    });

    test("a requested modelKey drives the base snapshot when live state has no model", () => {
        const sessionId = "ses-status-requested-model";
        const detail = buildStatusDetail(
            sessionId,
            process.cwd(),
            "test-provider/test-model",
            { execute_threshold_tokens: { "test-provider/test-model": 50_000 } },
            createLiveSessionState(),
            undefined,
            { usage: { current_total_input_tokens: 64_000, context_limit_tokens: 100_000 } },
        );
        expect(detail.contextLimit).toBe(100_000);
        expect(detail.native_context_usage_percentage).toBe(50);
        expect(detail.executeThresholdMode).toBe("tokens");
        expect(detail.executeThresholdTokens).toBe(50_000);

        const noDaemon = buildStatusDetail(
            sessionId,
            process.cwd(),
            "test-provider/test-model",
            undefined,
            createLiveSessionState(),
        );
        expect(noDaemon.contextLimit).toBe(128_000);
    });

    test("a requested modelKey overrides a stale live model without rewriting live state", () => {
        const sessionId = "ses-status-stale-live";
        const live = createLiveSessionState();
        live.liveModelBySession.set(sessionId, { providerID: "stale", modelID: "model" });
        const config = {
            execute_threshold_percentage: { default: 65, "test-provider/test-model": 50 },
            cache_ttl: { default: "5m", "test-provider/test-model": "10m" },
        };
        const detail = buildStatusDetail(
            sessionId,
            process.cwd(),
            "test-provider/test-model",
            config,
            live,
        );
        expect(detail.executeThreshold).toBe(50);
        expect(detail.cacheTtl).toBe("10m");
        expect(live.liveModelBySession.get(sessionId)).toEqual({
            providerID: "stale",
            modelID: "model",
        });
    });
});

describe("clearWorkMetricsCarry", () => {
    const originalXdgDataHome = process.env.XDG_DATA_HOME;
    let dataHome: string | undefined;

    afterEach(() => {
        closeReadOnlySessionDb();
        if (originalXdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
        else process.env.XDG_DATA_HOME = originalXdgDataHome;
        if (dataHome) rmSync(dataHome, { recursive: true, force: true });
        dataHome = undefined;
    });

    function openTempOpenCodeDb(): Database {
        dataHome = mkdtempSync(join(tmpdir(), "rpc-handlers-carry-"));
        const dbPath = join(dataHome, "opencode", "opencode.db");
        mkdirSync(dirname(dbPath), { recursive: true });
        const db = new Database(dbPath);
        db.exec(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL)",
        );
        process.env.XDG_DATA_HOME = dataHome;
        return db;
    }

    function insertAssistantRow(
        db: Database,
        sessionId: string,
        id: string,
        timeCreated: number,
        inputTokens: number,
    ): void {
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
        ).run(
            id,
            sessionId,
            timeCreated,
            JSON.stringify({
                id,
                role: "assistant",
                agent: "build",
                providerID: "test-provider",
                modelID: "test-model",
                time: { created: timeCreated, completed: timeCreated + 5 },
                tokens: { input: inputTokens, output: 10, cache: { read: 0, write: 0 } },
            }),
        );
    }

    test("usage lost from memory is recovered from the newest persisted response", () => {
        const sessionId = "ses-usage-recovered";
        const db = openTempOpenCodeDb();
        insertAssistantRow(db, sessionId, "a", 1, 10_000);
        insertAssistantRow(db, sessionId, "b", 2, 64_000);
        closeQuietly(db);

        // No live model and no live usage: both come back from the database.
        const live = createLiveSessionState();
        const snapshot = buildSidebarSnapshot(sessionId, process.cwd(), live);
        expect(snapshot.inputTokens).toBe(64_000);
        expect(snapshot.usagePercentage).toBe(50);
        const recovered = live.contextUsageBySession.get(sessionId);
        expect(recovered?.messageID).toBe("b");
        expect(recovered?.model).toEqual({ providerID: "test-provider", modelID: "test-model" });
        expect(recovered?.lastResponseTime).toBe(7);

        // The recovered entry is cached, so the dialog's countdown starts from the persisted response time.
        const detail = buildStatusDetail(sessionId, process.cwd(), undefined, undefined, live);
        expect(detail.lastResponseTime).toBe(7);
    });

    test("a retained carry survives row deletion until the session is cleared", () => {
        const sessionId = "ses-carry-clear";
        const db = openTempOpenCodeDb();
        insertAssistantRow(db, sessionId, "a", 1, 1_000);
        insertAssistantRow(db, sessionId, "b", 2, 2_000);

        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(2_000);

        // The carry holds row `a` as its watermark, so the next poll still reports `a` after the rows vanish.
        db.exec("DELETE FROM message");
        closeQuietly(db);
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(1_000);

        clearWorkMetricsCarry(sessionId);
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(0);
    });

    test("message.removed clears the carry so the next poll re-reads the session", async () => {
        const sessionId = "ses-carry-removed";
        const db = openTempOpenCodeDb();
        // A prompt drop at `b` closes a 3k phase, so the total is 3k + 2k while `a` exists and 2k once it is gone.
        insertAssistantRow(db, sessionId, "a", 1, 3_000);
        insertAssistantRow(db, sessionId, "b", 2, 1_000);
        insertAssistantRow(db, sessionId, "c", 3, 2_000);
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(5_000);

        db.exec("DELETE FROM message WHERE id = 'a'");
        closeQuietly(db);
        // The carry already folded `a`, so a poll without the event still reports the closed phase.
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(5_000);

        const handle = createEventHandler({ contextUsageMap: new Map() });
        await handle({
            event: {
                type: "message.removed",
                properties: { sessionID: sessionId, messageID: "a" },
            },
        });
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(2_000);
    });

    test("an update to a folded row clears the carry; an update to the held-back newest row keeps it", () => {
        const sessionId = "ses-carry-updated";
        const db = openTempOpenCodeDb();
        insertAssistantRow(db, sessionId, "a", 1, 3_000);
        insertAssistantRow(db, sessionId, "b", 2, 1_000);
        insertAssistantRow(db, sessionId, "c", 3, 2_000);
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(5_000);

        // Row `a` shrinks to 1.5k, so the phase `b` closes is worth 1.5k once re-read.
        db.prepare("UPDATE message SET data = ? WHERE id = 'a'").run(
            JSON.stringify({
                id: "a",
                role: "assistant",
                agent: "build",
                tokens: { input: 1_500, output: 10, cache: { read: 0, write: 0 } },
            }),
        );
        closeQuietly(db);

        // `c` is the held-back newest row (watermark is `b`), so an update to it leaves the carry alone.
        clearWorkMetricsCarryIfFolded(sessionId, "c");
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(5_000);

        clearWorkMetricsCarryIfFolded(sessionId, "a");
        expect(buildSidebarSnapshot(sessionId, process.cwd()).totalInputTokens).toBe(3_500);
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
});
