import { afterEach, describe, expect, test } from "bun:test";
import type { SidebarSnapshot } from "../shared/rpc-types";
import {
    applyStickySnapshotCache,
    clearSidebarSnapshotCache,
    resetSidebarSnapshotCache,
} from "./sidebar-snapshot-cache";

afterEach(() => {
    resetSidebarSnapshotCache();
});

function makeSnapshot(overrides: Partial<SidebarSnapshot> = {}): SidebarSnapshot {
    return {
        sessionId: "ses_test",
        usagePercentage: 0,
        inputTokens: 0,
        contextLimit: 0,
        systemPromptTokens: 0,
        compartmentCount: 0,
        memoryCount: 0,
        memoryState: "available",
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
        ...overrides,
    };
}

describe("applyStickySnapshotCache", () => {
    test("passes through fresh snapshot when inputTokens > 0 and caches it", () => {
        const fresh = makeSnapshot({
            inputTokens: 100_000,
            usagePercentage: 30,
            systemPromptTokens: 25_000,
            compartmentTokens: 50_000,
            conversationTokens: 25_000,
            compartmentCount: 5,
            memoryCount: 10,
        });
        const result = applyStickySnapshotCache("ses_test", fresh);
        expect(result).toEqual(fresh);
    });

    test("passes through zero snapshot when no prior cached value (true new session)", () => {
        const fresh = makeSnapshot({ inputTokens: 0 });
        const result = applyStickySnapshotCache("ses_test", fresh);
        expect(result.inputTokens).toBe(0);
    });

    test("returns hybrid (cached tokens + fresh counts) when inputTokens drops to 0 mid-turn", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 350_000,
                usagePercentage: 35,
                native_context_usage_percentage: 17.5,
                systemPromptTokens: 25_000,
                compartmentTokens: 128_000,
                factTokens: 200,
                memoryTokens: 8_000,
                docsTokens: 3_000,
                profileTokens: 1_800,
                conversationTokens: 53_000,
                toolCallTokens: 99_000,
                toolDefinitionTokens: 32_000,
                compartmentCount: 392,
                memoryCount: 486,
            }),
        );
        const flickered = makeSnapshot({
            inputTokens: 0, // mid-turn flicker
            native_context_usage_percentage: 0,
            compartmentCount: 393, // a new compartment landed
            memoryCount: 487,
            historianRunning: true,
            pendingOpsCount: 12,
            totalInputTokens: 9_000_000,
            newWorkTokens: 4_000,
        });
        const result = applyStickySnapshotCache("ses_test", flickered);

        // Every token-derived value comes from the cached snapshot.
        expect(result.inputTokens).toBe(350_000);
        expect(result.usagePercentage).toBe(35);
        expect(result.native_context_usage_percentage).toBe(17.5);
        expect(result.systemPromptTokens).toBe(25_000);
        expect(result.compartmentTokens).toBe(128_000);
        expect(result.factTokens).toBe(200);
        expect(result.memoryTokens).toBe(8_000);
        expect(result.docsTokens).toBe(3_000);
        expect(result.profileTokens).toBe(1_800);
        expect(result.conversationTokens).toBe(53_000);
        expect(result.toolCallTokens).toBe(99_000);
        expect(result.toolDefinitionTokens).toBe(32_000);

        // Counts, live state, and cumulative work metrics come from the fresh build.
        expect(result.compartmentCount).toBe(393);
        expect(result.memoryCount).toBe(487);
        expect(result.historianRunning).toBe(true);
        expect(result.pendingOpsCount).toBe(12);
        expect(result.totalInputTokens).toBe(9_000_000);
        expect(result.newWorkTokens).toBe(4_000);
    });

    test("restored breakdown stays consistent: buckets sum to the restored inputTokens", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 100_000,
                systemPromptTokens: 10_000,
                toolDefinitionTokens: 5_000,
                compartmentTokens: 30_000,
                factTokens: 1_000,
                memoryTokens: 4_000,
                docsTokens: 6_000,
                profileTokens: 2_000,
                conversationTokens: 30_000,
                toolCallTokens: 12_000,
            }),
        );
        const result = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 0, compartmentInProgress: true }),
        );
        const bucketSum =
            result.systemPromptTokens +
            result.toolDefinitionTokens +
            result.compartmentTokens +
            result.factTokens +
            result.memoryTokens +
            result.docsTokens +
            result.profileTokens +
            result.conversationTokens +
            result.toolCallTokens;
        expect(bucketSum).toBe(result.inputTokens);
    });

    test("clears cached tokens when zero snapshot drops counts too (real reset)", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 100_000,
                compartmentCount: 5,
                memoryCount: 10,
            }),
        );

        // The cache treats inputTokens, compartmentCount, and memoryCount of 0 with no in-flight signal as a reset.
        const reset = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 0,
                compartmentCount: 0,
                memoryCount: 0,
                compartmentInProgress: false,
                historianRunning: false,
            }),
        );
        expect(reset.inputTokens).toBe(0);

        // The reset removes the cached entry, so later in-flight signals cannot restore tokens.
        const later = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 0, compartmentInProgress: true }),
        );
        expect(later.inputTokens).toBe(0);
    });

    test("a complete available memory read with a lower count is reset evidence", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 100_000, compartmentCount: 5, memoryCount: 10 }),
        );
        const reset = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 0,
                compartmentCount: 5,
                memoryCount: 4,
                memoryState: "available",
            }),
        );
        expect(reset.inputTokens).toBe(0);

        const later = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 0, compartmentInProgress: true }),
        );
        expect(later.inputTokens).toBe(0);
    });

    test("a truncated memory read with a lower count is not reset evidence", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 100_000, compartmentCount: 5, memoryCount: 300 }),
        );
        // The capped read returns a prefix, so its count is a lower bound and can fall below the cached count.
        const result = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 0,
                compartmentCount: 5,
                memoryCount: 256,
                memoryTruncated: true,
            }),
        );
        expect(result.inputTokens).toBe(100_000);
        expect(result.memoryCount).toBe(256);
        expect(result.memoryTruncated).toBe(true);
    });

    test("a non-available memory state with a zero count is not reset evidence", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 100_000, compartmentCount: 5, memoryCount: 10 }),
        );
        for (const memoryState of ["unavailable:daemon_absent", "stale", "disabled", null]) {
            const result = applyStickySnapshotCache(
                "ses_test",
                makeSnapshot({
                    inputTokens: 0,
                    compartmentCount: 5,
                    memoryCount: 0,
                    memoryState,
                }),
            );
            expect(result.inputTokens).toBe(100_000);
            expect(result.memoryCount).toBe(0);
            expect(result.memoryState).toBe(memoryState);
        }
    });

    test("an indeterminate memory read does not rescue a compartment-count drop", () => {
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 100_000, compartmentCount: 5, memoryCount: 10 }),
        );
        const reset = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 0,
                compartmentCount: 0,
                memoryCount: 0,
                memoryState: "unavailable:daemon_absent",
            }),
        );
        expect(reset.inputTokens).toBe(0);
    });

    test("sticks during first-user-prompt flicker when counts survive", () => {
        // The cache preserves cached tokens when inputTokens is 0, counts match the cached snapshot, and no in-flight signals are set.
        applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({
                inputTokens: 350_000,
                usagePercentage: 35,
                systemPromptTokens: 25_000,
                compartmentTokens: 128_000,
                memoryTokens: 8_000,
                conversationTokens: 100_000,
                compartmentCount: 392,
                memoryCount: 486,
            }),
        );

        const firstPromptFlicker = makeSnapshot({
            inputTokens: 0,
            compartmentInProgress: false,
            historianRunning: false,
            pendingOpsCount: 0,
            compartmentCount: 392,
            memoryCount: 486,
        });
        const result = applyStickySnapshotCache("ses_test", firstPromptFlicker);

        expect(result.inputTokens).toBe(350_000);
        expect(result.compartmentTokens).toBe(128_000);
        expect(result.memoryTokens).toBe(8_000);
        expect(result.conversationTokens).toBe(100_000);
    });

    test("sticks when compartment work is explicitly in progress", () => {
        applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 100_000 }));
        const result = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 0, compartmentInProgress: true }),
        );
        expect(result.inputTokens).toBe(100_000);
    });

    test("does not stick after fresh non-zero overwrites the cached zero state", () => {
        applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 100_000 }));
        // The cache preserves token breakdowns during a mid-turn flicker.
        const stuck = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 0, compartmentInProgress: true }),
        );
        expect(stuck.inputTokens).toBe(100_000);
        // A nonzero inputTokens snapshot replaces the cached snapshot.
        const fresh = applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 200_000 }));
        expect(fresh.inputTokens).toBe(200_000);
        // A later zero-inputTokens snapshot uses the replacement.
        const stuck2 = applyStickySnapshotCache(
            "ses_test",
            makeSnapshot({ inputTokens: 0, compartmentInProgress: true }),
        );
        expect(stuck2.inputTokens).toBe(200_000);
    });

    test("does not bleed across sessions", () => {
        applyStickySnapshotCache(
            "ses_a",
            makeSnapshot({ sessionId: "ses_a", inputTokens: 100_000 }),
        );
        const result = applyStickySnapshotCache(
            "ses_b",
            makeSnapshot({ sessionId: "ses_b", inputTokens: 0 }),
        );
        expect(result.inputTokens).toBe(0);
        expect(result.sessionId).toBe("ses_b");
    });

    test("clearSidebarSnapshotCache removes cached entry for a session", () => {
        applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 100_000 }));
        clearSidebarSnapshotCache("ses_test");
        const result = applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 0 }));
        // Without a cached snapshot, zero inputTokens passes through unchanged.
        expect(result.inputTokens).toBe(0);
    });

    test("expires stale cached snapshot after age threshold", () => {
        // Cached snapshots expire after 5 minutes.
        const realNow = Date.now;
        const t0 = 1_000_000_000_000;
        Date.now = () => t0;
        applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 100_000 }));

        // Snapshots older than 5 minutes do not restore zero-token readings.
        Date.now = () => t0 + 6 * 60 * 1000;
        const result = applyStickySnapshotCache("ses_test", makeSnapshot({ inputTokens: 0 }));
        expect(result.inputTokens).toBe(0);

        Date.now = realNow;
    });
});
