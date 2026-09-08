/// <reference types="bun-types" />

import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    closeCompactionMarkerDb,
    EIDNARA_PROVIDER_ID,
    findBoundaryUserMessage,
    generateMessageId,
    injectCompactionMarker,
    listSessionCompactionMarkers,
    removeCompactionMarker,
    removeEidnaraOwnedCompactionMarkers,
    removeForeignCompactionMarker,
} from "./compaction-marker";

const tempDirs: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

function useTempDataHome(prefix: string): string {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
    mkdirSync(join(dir, "opencode"), { recursive: true });
    return dir;
}

function createOpenCodeTestDb(dataHome: string): Database {
    const db = new Database(join(dataHome, "opencode", "opencode.db"));
    db.exec("PRAGMA journal_mode=WAL");
    db.exec(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    db.exec(
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    return db;
}

function insertMessage(
    db: Database,
    id: string,
    role: string,
    timeCreated: number,
    data: Record<string, unknown> = {},
): void {
    db.prepare(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, 'ses-1', ?, ?, ?)",
    ).run(id, timeCreated, timeCreated, JSON.stringify({ role, ...data }));
}

function insertPart(
    db: Database,
    id: string,
    messageId: string,
    timeCreated: number,
    data: Record<string, unknown>,
): void {
    db.prepare(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, 'ses-1', ?, ?, ?)",
    ).run(id, messageId, timeCreated, timeCreated, JSON.stringify(data));
}

function insertNativeCompactionLineage(
    db: Database,
    boundaryId: string,
    summaryId: string,
    timeCreated: number,
): void {
    insertMessage(db, boundaryId, "user", timeCreated);
    insertPart(db, `prt_${boundaryId}`, boundaryId, timeCreated, {
        type: "compaction",
        auto: false,
    });
    insertMessage(db, summaryId, "assistant", timeCreated + 1, {
        parentID: boundaryId,
        summary: true,
        finish: "stop",
        providerID: "anthropic",
        modelID: "claude-opus-4-8",
    });
    insertPart(db, `prt_${summaryId}`, summaryId, timeCreated + 1, {
        type: "text",
        text: "native summary",
    });
}

function countRows(dataHome: string, table: "message" | "part"): number {
    const db = new Database(join(dataHome, "opencode", "opencode.db"), { readonly: true });
    try {
        const row = db.prepare(`SELECT COUNT(*) AS n FROM ${table}`).get() as { n: number };
        return row.n;
    } finally {
        closeQuietly(db);
    }
}

afterEach(() => {
    closeCompactionMarkerDb();
    process.env.XDG_DATA_HOME = originalXdgDataHome;
    for (const dir of tempDirs) {
        rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    }
    tempDirs.length = 0;
});

describe("findBoundaryUserMessage", () => {
    it("anchors by endMessageId after rows before the target were deleted", () => {
        const dataHome = useTempDataHome("marker-boundary-deleted-before-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_deleted_user", "user", 100);
        insertMessage(db, "msg_002_deleted_assistant", "assistant", 200);
        insertMessage(db, "msg_003_prior_user", "user", 300);
        insertMessage(db, "msg_004_target", "assistant", 400);
        insertMessage(db, "msg_005_after_user", "user", 500);
        db.prepare(
            "DELETE FROM message WHERE id IN ('msg_001_deleted_user', 'msg_002_deleted_assistant')",
        ).run();
        closeQuietly(db);

        expect(findBoundaryUserMessage("ses-1", "msg_004_target")?.id).toBe("msg_003_prior_user");
    });

    it("uses the canonical time_created/id tie-break at equal timestamps", () => {
        const dataHome = useTempDataHome("marker-boundary-tiebreak-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_a_prior_user", "user", 1_000);
        insertMessage(db, "msg_b_target", "assistant", 1_000);
        insertMessage(db, "msg_c_after_user", "user", 1_000);
        closeQuietly(db);

        expect(findBoundaryUserMessage("ses-1", "msg_b_target")?.id).toBe("msg_a_prior_user");
    });

    it("returns the target itself when the target message is a user", () => {
        const dataHome = useTempDataHome("marker-boundary-target-user-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_prior_user", "user", 100);
        insertMessage(db, "msg_002_target_user", "user", 200);
        closeQuietly(db);

        expect(findBoundaryUserMessage("ses-1", "msg_002_target_user")?.id).toBe(
            "msg_002_target_user",
        );
    });

    it("is unchanged by deleting rows after the target", () => {
        const dataHome = useTempDataHome("marker-boundary-deleted-after-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_prior_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        insertMessage(db, "msg_003_after_user", "user", 300);
        closeQuietly(db);

        expect(findBoundaryUserMessage("ses-1", "msg_002_target")?.id).toBe("msg_001_prior_user");

        const reopened = new Database(join(dataHome, "opencode", "opencode.db"));
        reopened.prepare("DELETE FROM message WHERE id = 'msg_003_after_user'").run();
        closeQuietly(reopened);
        closeCompactionMarkerDb();

        expect(findBoundaryUserMessage("ses-1", "msg_002_target")?.id).toBe("msg_001_prior_user");
    });

    it("finds a prior user across a long assistant/tool span", () => {
        const dataHome = useTempDataHome("marker-boundary-long-span-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_prior_user", "user", 100);
        for (let i = 0; i < 150; i++) {
            insertMessage(
                db,
                `msg_${String(i + 2).padStart(3, "0")}_assistant`,
                "assistant",
                101 + i,
            );
        }
        insertMessage(db, "msg_999_target", "tool", 1_000);
        closeQuietly(db);

        expect(findBoundaryUserMessage("ses-1", "msg_999_target")?.id).toBe("msg_001_prior_user");
    });
});

describe("injectCompactionMarker", () => {
    it("keeps deterministic marker ids in OpenCode's lexicographic row order", () => {
        const dataHome = useTempDataHome("marker-inject-id-order-");
        const db = createOpenCodeTestDb(dataHome);
        const boundaryId = generateMessageId(1_000, 0n, "boundary");
        const retainedId = generateMessageId(1_002, 0n, "retained");
        insertMessage(db, boundaryId, "user", 1_000);
        insertMessage(db, retainedId, "assistant", 1_002);
        closeQuietly(db);

        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: retainedId,
            summaryText: "summary placeholder",
            directory: dataHome,
        });

        expect(result?.summaryMessageId).toMatch(/^msg_[0-9a-f]{12}[0-9A-Za-z]{14}$/);
        expect(result?.compactionPartId).toMatch(/^prt_[0-9a-f]{12}[0-9A-Za-z]{14}$/);
        expect(boundaryId < (result?.summaryMessageId ?? "")).toBe(true);
        expect((result?.summaryMessageId ?? "") < retainedId).toBe(true);

        const inspection = new Database(join(dataHome, "opencode", "opencode.db"));
        const rows = inspection
            .prepare(
                "SELECT id, json_extract(data, '$.role') AS role, json_extract(data, '$.summary') AS summary, json_extract(data, '$.parentID') AS parentID FROM message WHERE session_id = 'ses-1' ORDER BY time_created ASC, id ASC",
            )
            .all() as Array<{
            id: string;
            role: string;
            summary: number | null;
            parentID: string | null;
        }>;
        expect(rows).toEqual([
            { id: boundaryId, role: "user", summary: null, parentID: null },
            {
                id: result?.summaryMessageId,
                role: "assistant",
                summary: 1,
                parentID: boundaryId,
            },
            { id: retainedId, role: "assistant", summary: null, parentID: null },
        ]);
        closeQuietly(inspection);
    });

    it("preserves the deterministic boundary in the healthy no-deletion case", () => {
        const dataHome = useTempDataHome("marker-inject-healthy-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_assistant", "assistant", 200);
        insertMessage(db, "msg_003_target", "assistant", 300);
        closeQuietly(db);

        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 3,
            endMessageId: "msg_003_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });

        expect(result?.boundaryMessageId).toBe("msg_001_user");
        expect(result?.summaryMessageId).toMatch(/^msg_[0-9a-f]{12}[0-9A-Za-z]{14}$/);

        const retry = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 3,
            endMessageId: "msg_003_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        expect(retry).toEqual(result);
    });

    it("writes the eidnara provider identity that the ownership queries match", () => {
        const dataHome = useTempDataHome("marker-inject-provider-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!result) throw new Error("injection returned null");

        const inspection = new Database(join(dataHome, "opencode", "opencode.db"), {
            readonly: true,
        });
        const row = inspection
            .prepare(
                "SELECT json_extract(data, '$.providerID') AS providerID FROM message WHERE id = ?",
            )
            .get(result.summaryMessageId) as { providerID: string };
        closeQuietly(inspection);
        expect(row.providerID).toBe(EIDNARA_PROVIDER_ID);

        expect(listSessionCompactionMarkers("ses-1")).toEqual([
            {
                compactionPartId: result.compactionPartId,
                boundaryMessageId: "msg_001_user",
                summaryMessageIds: [result.summaryMessageId],
            },
        ]);
    });

    it("writes nothing when the resolved boundary was deleted before the transaction", () => {
        const dataHome = useTempDataHome("marker-inject-stale-boundary-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const resolvedBoundary = findBoundaryUserMessage("ses-1", "msg_002_target");
        expect(resolvedBoundary?.id).toBe("msg_001_user");

        const reverter = new Database(join(dataHome, "opencode", "opencode.db"));
        reverter.prepare("DELETE FROM message WHERE id = 'msg_001_user'").run();
        closeQuietly(reverter);

        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
            resolvedBoundary: resolvedBoundary ?? undefined,
        });

        expect(result).toBeNull();
        expect(countRows(dataHome, "message")).toBe(1);
        expect(countRows(dataHome, "part")).toBe(0);
    });

    it("keeps a native automatic compaction part on the boundary when replacing a stale lineage", () => {
        const dataHome = useTempDataHome("marker-inject-stale-lineage-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_assistant", "assistant", 200);
        insertMessage(db, "msg_003_target", "assistant", 300);
        // A native automatic compaction at the boundary must survive stale-lineage cleanup.
        insertPart(db, "prt_native_compaction", "msg_001_user", 100, {
            type: "compaction",
            auto: true,
        });
        closeQuietly(db);

        const stale = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_assistant",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!stale) throw new Error("first injection returned null");
        // Injecting a marker through `msg_003_target` replaces the stale lineage ending at `msg_002_assistant`.
        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 3,
            endMessageId: "msg_003_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!result) throw new Error("second injection returned null");
        expect(result.summaryMessageId).not.toBe(stale.summaryMessageId);

        const inspection = new Database(join(dataHome, "opencode", "opencode.db"), {
            readonly: true,
        });
        const partIds = (
            inspection
                .prepare("SELECT id FROM part WHERE message_id = 'msg_001_user' ORDER BY id")
                .all() as Array<{ id: string }>
        ).map((row) => row.id);
        const staleSummary = inspection
            .prepare("SELECT 1 AS one FROM message WHERE id = ?")
            .get(stale.summaryMessageId);
        closeQuietly(inspection);

        expect(partIds).toEqual([result.compactionPartId, "prt_native_compaction"].sort());
        expect(staleSummary).toBeNull();
    });

    it("retains a stale lineage whose summary a surviving tail_start_id references", () => {
        const dataHome = useTempDataHome("marker-inject-stale-retained-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_assistant", "assistant", 200);
        insertMessage(db, "msg_003_target", "assistant", 300);
        closeQuietly(db);

        const stale = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_assistant",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!stale) throw new Error("first injection returned null");

        const native = new Database(join(dataHome, "opencode", "opencode.db"));
        insertMessage(native, "msg_005_user", "user", 500);
        insertPart(native, "prt_005_native", "msg_005_user", 500, {
            type: "compaction",
            auto: true,
            tail_start_id: stale.summaryMessageId,
        });
        closeQuietly(native);
        closeCompactionMarkerDb();

        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 3,
            endMessageId: "msg_003_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!result) throw new Error("second injection returned null");

        const inspection = new Database(join(dataHome, "opencode", "opencode.db"), {
            readonly: true,
        });
        const summaryIds = (
            inspection
                .prepare(
                    "SELECT id FROM message WHERE json_extract(data, '$.parentID') = 'msg_001_user' ORDER BY id",
                )
                .all() as Array<{ id: string }>
        ).map((row) => row.id);
        closeQuietly(inspection);
        expect(summaryIds).toEqual([stale.summaryMessageId, result.summaryMessageId].sort());
    });

    it("writes nothing when the end target was deleted before the transaction", () => {
        const dataHome = useTempDataHome("marker-inject-stale-target-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const resolvedBoundary = findBoundaryUserMessage("ses-1", "msg_002_target");
        expect(resolvedBoundary?.id).toBe("msg_001_user");

        const reverter = new Database(join(dataHome, "opencode", "opencode.db"));
        reverter.prepare("DELETE FROM message WHERE id = 'msg_002_target'").run();
        closeQuietly(reverter);

        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
            resolvedBoundary: resolvedBoundary ?? undefined,
        });

        expect(result).toBeNull();
        expect(countRows(dataHome, "message")).toBe(1);
        expect(countRows(dataHome, "part")).toBe(0);
    });

    it("treats a message with malformed JSON as a non-match instead of failing", () => {
        const dataHome = useTempDataHome("marker-inject-malformed-json-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('msg_002_broken', 'ses-1', 200, 200, '{not json')",
        ).run();
        insertMessage(db, "msg_003_target", "assistant", 300);
        closeQuietly(db);

        expect(findBoundaryUserMessage("ses-1", "msg_003_target")?.id).toBe("msg_001_user");
        const result = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 3,
            endMessageId: "msg_003_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        expect(result?.boundaryMessageId).toBe("msg_001_user");
        expect(removeEidnaraOwnedCompactionMarkers("ses-1", "summary placeholder")).toEqual({
            verified: true,
            removedLineages: 1,
            removedRows: 3,
            retainedLineages: 0,
        });
    });
});

describe("listSessionCompactionMarkers", () => {
    it("never lists a native compaction boundary as a removable marker", () => {
        const dataHome = useTempDataHome("marker-list-native-");
        const db = createOpenCodeTestDb(dataHome);
        insertNativeCompactionLineage(db, "msg_001_native_user", "msg_002_native_summary", 100);
        insertMessage(db, "msg_003_user", "user", 300);
        // A compaction part carrying `tail_start_id` is a newer native shape with no summary of its own.
        insertPart(db, "prt_003_tail", "msg_003_user", 300, {
            type: "compaction",
            auto: true,
            tail_start_id: "msg_003_user",
        });
        closeQuietly(db);

        expect(listSessionCompactionMarkers("ses-1")).toEqual([]);
    });

    it("lists only the compaction part whose id derives from an eidnara summary on its boundary", () => {
        const dataHome = useTempDataHome("marker-list-owned-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        // An orphan compaction part with no summary is never owned.
        insertPart(db, "prt_001_orphan", "msg_001_user", 100, { type: "compaction", auto: true });
        insertMessage(db, "msg_002_user", "user", 200);
        insertMessage(db, "msg_003_target", "assistant", 300);
        // A native part next to a plugin summary has the plugin payload but not the plugin id.
        insertPart(db, "prt_002_native", "msg_002_user", 200, { type: "compaction", auto: true });
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 3,
            endMessageId: "msg_003_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");
        expect(injected.boundaryMessageId).toBe("msg_002_user");

        expect(listSessionCompactionMarkers("ses-1")).toEqual([
            {
                compactionPartId: injected.compactionPartId,
                boundaryMessageId: "msg_002_user",
                summaryMessageIds: [injected.summaryMessageId],
            },
        ]);
    });

    it("removes a listed foreign marker together with its summary rows", () => {
        const dataHome = useTempDataHome("marker-list-remove-");
        const db = createOpenCodeTestDb(dataHome);
        insertNativeCompactionLineage(db, "msg_001_native_user", "msg_002_native_summary", 100);
        insertMessage(db, "msg_003_user", "user", 300);
        insertMessage(db, "msg_004_target", "assistant", 400);
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 4,
            endMessageId: "msg_004_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");

        const markers = listSessionCompactionMarkers("ses-1");
        expect(markers.map((marker) => marker.compactionPartId)).toEqual([
            injected.compactionPartId,
        ]);
        expect(removeForeignCompactionMarker("ses-1", markers[0], null)).toBe("removed");

        expect(listSessionCompactionMarkers("ses-1")).toEqual([]);
        // Native rows plus the two plain messages survive; every injected row is gone.
        expect(countRows(dataHome, "message")).toBe(4);
        expect(countRows(dataHome, "part")).toBe(2);
    });

    it("retains a foreign marker whose summary a surviving tail_start_id references", () => {
        const dataHome = useTempDataHome("marker-list-retained-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");

        const native = new Database(join(dataHome, "opencode", "opencode.db"));
        insertMessage(native, "msg_005_user", "user", 500);
        insertPart(native, "prt_005_native", "msg_005_user", 500, {
            type: "compaction",
            auto: true,
            tail_start_id: injected.summaryMessageId,
        });
        closeQuietly(native);
        closeCompactionMarkerDb();

        const [marker] = listSessionCompactionMarkers("ses-1");
        if (!marker) throw new Error("expected the injected marker to be listed");
        expect(removeForeignCompactionMarker("ses-1", marker, null)).toBe("retained");
        expect(countRows(dataHome, "message")).toBe(4);
        expect(countRows(dataHome, "part")).toBe(3);
    });
});

describe("removeEidnaraOwnedCompactionMarkers", () => {
    it("removes the plugin lineage, leaves native rows, and reports zeros on the second run", () => {
        const dataHome = useTempDataHome("marker-flip-off-");
        const db = createOpenCodeTestDb(dataHome);
        insertNativeCompactionLineage(db, "msg_001_native_user", "msg_002_native_summary", 100);
        insertMessage(db, "msg_003_user", "user", 300);
        insertMessage(db, "msg_004_target", "assistant", 400);
        closeQuietly(db);

        expect(
            injectCompactionMarker({
                sessionId: "ses-1",
                endOrdinal: 4,
                endMessageId: "msg_004_target",
                summaryText: "summary placeholder",
                directory: dataHome,
            }),
        ).not.toBeNull();

        const first = removeEidnaraOwnedCompactionMarkers("ses-1", "summary placeholder");
        expect(first).toEqual({
            verified: true,
            removedLineages: 1,
            removedRows: 3,
            retainedLineages: 0,
        });
        expect(countRows(dataHome, "message")).toBe(4);
        expect(countRows(dataHome, "part")).toBe(2);

        const second = removeEidnaraOwnedCompactionMarkers("ses-1", "summary placeholder");
        expect(second).toEqual({
            verified: true,
            removedLineages: 0,
            removedRows: 0,
            retainedLineages: 0,
        });
    });

    it("leaves a native compaction part on the same boundary as the removed plugin lineage", () => {
        const dataHome = useTempDataHome("marker-flip-off-shared-boundary-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        insertPart(db, "prt_001_native", "msg_001_user", 100, { type: "compaction", auto: true });
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");

        expect(removeEidnaraOwnedCompactionMarkers("ses-1", "summary placeholder")).toEqual({
            verified: true,
            removedLineages: 1,
            removedRows: 3,
            retainedLineages: 0,
        });

        const inspection = new Database(join(dataHome, "opencode", "opencode.db"), {
            readonly: true,
        });
        const partIds = (
            inspection.prepare("SELECT id FROM part ORDER BY id").all() as Array<{ id: string }>
        ).map((row) => row.id);
        closeQuietly(inspection);
        expect(partIds).toEqual(["prt_001_native"]);
    });

    it("retains a lineage that a surviving tail_start_id references", () => {
        const dataHome = useTempDataHome("marker-flip-off-retained-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        expect(injected).not.toBeNull();

        const native = new Database(join(dataHome, "opencode", "opencode.db"));
        insertMessage(native, "msg_005_user", "user", 500);
        insertPart(native, "prt_005_native", "msg_005_user", 500, {
            type: "compaction",
            auto: true,
            tail_start_id: injected?.summaryMessageId,
        });
        closeQuietly(native);
        closeCompactionMarkerDb();

        const result = removeEidnaraOwnedCompactionMarkers("ses-1", "summary placeholder");
        expect(result).toEqual({
            verified: false,
            removedLineages: 0,
            removedRows: 0,
            retainedLineages: 1,
        });
        expect(countRows(dataHome, "message")).toBe(4);
        expect(countRows(dataHome, "part")).toBe(3);
    });
});

describe("removeCompactionMarker", () => {
    it("removes the injected lineage by state and leaves the rest of the session", () => {
        const dataHome = useTempDataHome("marker-remove-direct-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");

        expect(removeCompactionMarker(injected)).toBe("removed");
        expect(countRows(dataHome, "message")).toBe(2);
        expect(countRows(dataHome, "part")).toBe(0);
        expect(removeCompactionMarker(injected)).toBe("removed");
    });

    it("retains the lineage when a surviving tail_start_id references its summary", () => {
        const dataHome = useTempDataHome("marker-remove-direct-retained-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");

        const native = new Database(join(dataHome, "opencode", "opencode.db"));
        insertMessage(native, "msg_005_user", "user", 500);
        insertPart(native, "prt_005_native", "msg_005_user", 500, {
            type: "compaction",
            auto: true,
            tail_start_id: injected.summaryMessageId,
        });
        closeQuietly(native);
        closeCompactionMarkerDb();

        expect(removeCompactionMarker(injected)).toBe("retained");
        expect(countRows(dataHome, "message")).toBe(4);
        expect(countRows(dataHome, "part")).toBe(3);
    });
});

describe("fork hygiene", () => {
    /** Copies every ses-1 row into ses-2 under new primary keys, remapping `parentID` the way `/fork` does. */
    function forkSession(dataHome: string): void {
        const db = new Database(join(dataHome, "opencode", "opencode.db"));
        const messages = db
            .prepare(
                "SELECT id, time_created, time_updated, data FROM message WHERE session_id = 'ses-1'",
            )
            .all() as Array<{
            id: string;
            time_created: number;
            time_updated: number;
            data: string;
        }>;
        const idMap = new Map(messages.map((row) => [row.id, `fork_${row.id}`]));
        for (const row of messages) {
            const data = JSON.parse(row.data) as Record<string, unknown>;
            if (typeof data.parentID === "string")
                data.parentID = idMap.get(data.parentID) ?? data.parentID;
            db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, 'ses-2', ?, ?, ?)",
            ).run(idMap.get(row.id), row.time_created, row.time_updated, JSON.stringify(data));
        }
        const parts = db
            .prepare(
                "SELECT id, message_id, time_created, time_updated, data FROM part WHERE session_id = 'ses-1'",
            )
            .all() as Array<{
            id: string;
            message_id: string;
            time_created: number;
            time_updated: number;
            data: string;
        }>;
        for (const row of parts) {
            db.prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, 'ses-2', ?, ?, ?)",
            ).run(
                `fork_${row.id}`,
                idMap.get(row.message_id),
                row.time_created,
                row.time_updated,
                row.data,
            );
        }
        closeQuietly(db);
        closeCompactionMarkerDb();
    }

    it("lists and removes a copied marker whose ids were remapped by the fork", () => {
        const dataHome = useTempDataHome("marker-fork-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertMessage(db, "msg_002_target", "assistant", 200);
        closeQuietly(db);

        const injected = injectCompactionMarker({
            sessionId: "ses-1",
            endOrdinal: 2,
            endMessageId: "msg_002_target",
            summaryText: "summary placeholder",
            directory: dataHome,
        });
        if (!injected) throw new Error("injection returned null");
        forkSession(dataHome);

        const forked = listSessionCompactionMarkers("ses-2");
        expect(forked).toEqual([
            {
                compactionPartId: `fork_${injected.compactionPartId}`,
                boundaryMessageId: "fork_msg_001_user",
                summaryMessageIds: [`fork_${injected.summaryMessageId}`],
            },
        ]);
        expect(removeForeignCompactionMarker("ses-2", forked[0], null)).toBe("removed");
        expect(listSessionCompactionMarkers("ses-2")).toEqual([]);
        // The parent session's lineage is untouched.
        expect(listSessionCompactionMarkers("ses-1")).toHaveLength(1);
    });
});

describe("write connection lifecycle", () => {
    it("closes a handle whose initialization pragma throws instead of leaking it", () => {
        const dataHome = useTempDataHome("marker-open-failure-");
        closeQuietly(createOpenCodeTestDb(dataHome));
        const closes: number[] = [];
        const originalExec = Database.prototype.exec;
        const originalClose = Database.prototype.close;
        const execSpy = spyOn(Database.prototype, "exec").mockImplementation(function (
            this: Database,
            sql: string,
        ) {
            if (sql.includes("journal_mode")) throw new Error("simulated pragma failure");
            return originalExec.call(this, sql);
        });
        const closeSpy = spyOn(Database.prototype, "close").mockImplementation(function (
            this: Database,
        ) {
            closes.push(1);
            return originalClose.call(this);
        });
        try {
            expect(() => listSessionCompactionMarkers("ses-1")).toThrow("simulated pragma failure");
            expect(closes).toHaveLength(1);
        } finally {
            execSpy.mockRestore();
            closeSpy.mockRestore();
        }
        // The failed handle was never cached, so the next call opens a fresh connection and succeeds.
        expect(listSessionCompactionMarkers("ses-1")).toEqual([]);
    });
});
