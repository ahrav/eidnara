import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    generateMessageId,
    injectCompactionMarker,
} from "../../features/context/compaction-marker";
import { _resetHarnessForTesting, setHarness } from "../../shared/harness";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    closeCompactionMarkerConnection,
    MARKER_SUMMARY_TEXT,
    removeCompactionMarkerForSession,
} from "./compaction-marker-manager";

const originalXdgDataHome = process.env.XDG_DATA_HOME;
let dataHome: string;

function insertMessage(db: Database, id: string, role: string, timeCreated: number): void {
    db.prepare(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, 'ses-1', ?, ?, ?)",
    ).run(id, timeCreated, timeCreated, JSON.stringify({ role }));
}

function openCodeDb(): Database {
    return new Database(join(dataHome, "opencode", "opencode.db"));
}

function rowCounts(): { messages: number; parts: number } {
    const db = openCodeDb();
    try {
        const messages = db.prepare("SELECT COUNT(*) AS n FROM message").get() as { n: number };
        const parts = db.prepare("SELECT COUNT(*) AS n FROM part").get() as { n: number };
        return { messages: messages.n, parts: parts.n };
    } finally {
        closeQuietly(db);
    }
}

beforeEach(() => {
    dataHome = mkdtempSync(join(tmpdir(), "marker-manager-"));
    process.env.XDG_DATA_HOME = dataHome;
    mkdirSync(join(dataHome, "opencode"), { recursive: true });
    const db = openCodeDb();
    db.exec("PRAGMA journal_mode=WAL");
    db.exec(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    db.exec(
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    insertMessage(db, generateMessageId(1_000, 0n, "boundary"), "user", 1_000);
    insertMessage(db, generateMessageId(1_002, 0n, "retained"), "assistant", 1_002);
    closeQuietly(db);
    setHarness("opencode");
});

afterEach(() => {
    closeCompactionMarkerConnection();
    _resetHarnessForTesting();
    process.env.XDG_DATA_HOME = originalXdgDataHome;
    rmSync(dataHome, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
});

function injectPluginMarker(): void {
    const injected = injectCompactionMarker({
        sessionId: "ses-1",
        endOrdinal: 2,
        endMessageId: generateMessageId(1_002, 0n, "retained"),
        summaryText: MARKER_SUMMARY_TEXT,
        directory: dataHome,
    });
    expect(injected).not.toBeNull();
}

describe("removeCompactionMarkerForSession", () => {
    it("removes the plugin-owned marker lineage and leaves the user history", () => {
        injectPluginMarker();
        const before = rowCounts();
        expect(before.messages).toBe(3);
        expect(before.parts).toBe(2);

        removeCompactionMarkerForSession("ses-1");

        expect(rowCounts()).toEqual({ messages: 2, parts: 0 });
    });

    it("is idempotent when the session owns no marker", () => {
        removeCompactionMarkerForSession("ses-1");
        expect(rowCounts()).toEqual({ messages: 2, parts: 0 });
    });

    it("does nothing outside the opencode harness", () => {
        injectPluginMarker();
        _resetHarnessForTesting();
        setHarness("pi");

        removeCompactionMarkerForSession("ses-1");

        expect(rowCounts()).toEqual({ messages: 3, parts: 2 });
    });
});
