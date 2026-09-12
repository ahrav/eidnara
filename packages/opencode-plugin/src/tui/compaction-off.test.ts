import { expect, test } from "bun:test";
import type { SidebarSnapshot } from "../shared/rpc-types";
import {
    compactionOffSidebarRows,
    nativeCompactionContextLabel,
    nativeContextLimit,
} from "./compaction-off";

function snapshot(overrides: Partial<SidebarSnapshot> = {}): SidebarSnapshot {
    return {
        sessionId: "ses-native-ui",
        usagePercentage: 63.1,
        inputTokens: 41_000,
        contextLimit: 100_000,
        native_context_usage_percentage: 41,
        compaction_enabled: false,
        systemPromptTokens: 0,
        compartmentCount: 12,
        archivedCompartmentCount: 3,
        memoryCount: 5,
        memoryState: "available",
        memoryBlockCount: 2,
        pendingOpsCount: 4,
        historianRunning: true,
        compartmentInProgress: true,
        sessionNoteCount: 2,
        readySmartNoteCount: 1,
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

test("uses the native-window percentage, not inputTokens over the reserved contextLimit", () => {
    // 40k tokens: 100k native window → 40%; the 80k output-reserved limit would read 50% and the threshold-relative usagePercentage 63.1%.
    const value = nativeCompactionContextLabel(
        snapshot({
            inputTokens: 40_000,
            contextLimit: 80_000,
            native_context_usage_percentage: 40,
        }),
    );

    expect(value).toBe("Context: 40.0% · native compaction");
});

test("falls back to inputTokens over contextLimit when the native percentage is absent", () => {
    const value = nativeCompactionContextLabel(
        snapshot({
            inputTokens: 40_000,
            contextLimit: 80_000,
            native_context_usage_percentage: undefined,
        }),
    );

    expect(value).toBe("Context: 50.0% · native compaction");
    expect(
        nativeCompactionContextLabel(
            snapshot({ contextLimit: 0, native_context_usage_percentage: undefined }),
        ),
    ).toBe("Context: unknown · native compaction");
});

test("names no owner when the host reports neither compaction.auto nor compaction.prune", () => {
    expect(nativeCompactionContextLabel(snapshot({ native_compaction_active: false }))).toBe(
        "Context: 41.0% · no active compaction",
    );
    expect(
        nativeCompactionContextLabel(
            snapshot({
                contextLimit: 0,
                native_context_usage_percentage: undefined,
                native_compaction_active: false,
            }),
        ),
    ).toBe("Context: unknown · no active compaction");
    // A producer that did not resolve the host setting keeps the native-compaction wording.
    expect(nativeCompactionContextLabel(snapshot({ native_compaction_active: undefined }))).toBe(
        "Context: 41.0% · native compaction",
    );
    expect(nativeCompactionContextLabel(snapshot({ native_compaction_active: true }))).toBe(
        "Context: 41.0% · native compaction",
    );
});

test("keeps historical compartments as a static archived row", () => {
    const initialRows = compactionOffSidebarRows(snapshot());
    const activeCountChangedRows = compactionOffSidebarRows(snapshot({ compartmentCount: 99 }));

    expect(initialRows).toEqual([
        { label: "Memories", value: "5" },
        { label: "Notes", value: "2" },
        { label: "Archived compartments", value: "3" },
    ]);
    expect(activeCountChangedRows).toEqual(initialRows);
});

test("hides the Notes and Archived rows when their counts are zero or absent", () => {
    const rows = compactionOffSidebarRows(
        snapshot({ sessionNoteCount: 0, archivedCompartmentCount: undefined }),
    );

    expect(rows).toEqual([{ label: "Memories", value: "5" }]);
});

test("shows the memory state instead of a zero count when memory is not available", () => {
    const disabled = compactionOffSidebarRows(
        snapshot({ memoryCount: 0, memoryState: "disabled", sessionNoteCount: 0 }),
    );
    const absent = compactionOffSidebarRows(
        snapshot({ memoryCount: 0, memoryState: "unavailable:daemon_absent", sessionNoteCount: 0 }),
    );

    expect(disabled[0]).toEqual({ label: "Memories", value: "disabled" });
    expect(absent[0]).toEqual({ label: "Memories", value: "unavailable:daemon_absent" });
    expect(compactionOffSidebarRows(snapshot({ memoryTruncated: true }))[0]).toEqual({
        label: "Memories",
        value: "5+",
    });
});

test("recovers the unreserved window from the native percentage for the token total", () => {
    // 40k tokens at 40% is a 100k window; the reserved contextLimit would pair 40K with 80K.
    expect(
        nativeContextLimit(
            snapshot({
                inputTokens: 40_000,
                contextLimit: 80_000,
                native_context_usage_percentage: 40,
            }),
        ),
    ).toBe(100_000);
    expect(
        nativeContextLimit(
            snapshot({
                inputTokens: 41_000,
                contextLimit: 160_000,
                native_context_usage_percentage: (41_000 / 200_000) * 100,
            }),
        ),
    ).toBe(200_000);
});

test("falls back to contextLimit when the native percentage is absent or zero", () => {
    expect(
        nativeContextLimit(
            snapshot({
                inputTokens: 40_000,
                contextLimit: 80_000,
                native_context_usage_percentage: undefined,
            }),
        ),
    ).toBe(80_000);
    expect(
        nativeContextLimit(
            snapshot({ inputTokens: 0, contextLimit: 80_000, native_context_usage_percentage: 0 }),
        ),
    ).toBe(80_000);
});
