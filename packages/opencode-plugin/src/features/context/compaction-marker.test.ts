/// <reference types="bun-types" />

import { afterEach, describe, expect, it } from "bun:test";
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

    it("lists a plugin marker only when its boundary carries an eidnara summary", () => {
        const dataHome = useTempDataHome("marker-list-owned-");
        const db = createOpenCodeTestDb(dataHome);
        insertMessage(db, "msg_001_user", "user", 100);
        insertPart(db, "prt_001_orphan", "msg_001_user", 100, { type: "compaction", auto: true });
        insertMessage(db, "msg_002_user", "user", 200);
        insertPart(db, "prt_002_owned", "msg_002_user", 200, { type: "compaction", auto: true });
        insertMessage(db, "msg_003_summary", "assistant", 201, {
            parentID: "msg_002_user",
            summary: true,
            finish: "stop",
            providerID: EIDNARA_PROVIDER_ID,
        });
        closeQuietly(db);

        expect(listSessionCompactionMarkers("ses-1")).toEqual([
            {
                compactionPartId: "prt_002_owned",
                boundaryMessageId: "msg_002_user",
                summaryMessageIds: ["msg_003_summary"],
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
        expect(removeForeignCompactionMarker("ses-1", markers[0], null)).toBe(true);

        expect(listSessionCompactionMarkers("ses-1")).toEqual([]);
        // Native rows plus the two plain messages survive; every injected row is gone.
        expect(countRows(dataHome, "message")).toBe(4);
        expect(countRows(dataHome, "part")).toBe(2);
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
