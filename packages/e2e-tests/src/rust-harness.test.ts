import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { deleteMessagesAfter, parseRustPassLine } from "./rust-harness";

// One line in the exact shape `rust-mode-transform.ts` logs, so a format drift fails here
// instead of silently zeroing a timing the perf suite bounds.
const PASS_LINE =
    "[eidnara] rust pass: decision=DEFER reason=steady served_from=transform in=12 out=12 " +
    "applied=true row_version=7 emergency_wait=1234.5 elapsed=41.7 ms module=23.4 ms stages=prefix_guard:6.2 " +
    "ordinal_resolve:1.1 clone:0.4 wire_build:3.9 wire_messages:3 transport:9.8 " +
    "transport_pages:1 transport_bytes:20480 apply:1.2 other:0.8";

describe("parseRustPassLine", () => {
    it("reads top-level fields and every stage timing, including the first stage after stages=", () => {
        const pass = parseRustPassLine(PASS_LINE);
        expect(pass).not.toBeNull();
        expect(pass).toMatchObject({
            decision: "DEFER",
            reason: "steady",
            emergencyWaitMs: 1234.5,
            servedFrom: "transform",
            inputCount: 12,
            outputCount: 12,
            applied: true,
            rowVersion: 7,
            elapsedMs: 41.7,
            moduleElapsedMs: 23.4,
            prefixGuardMs: 6.2,
            wireBuildMs: 3.9,
            wireMessages: 3,
            transportMs: 9.8,
            transportPages: 1,
            transportBytes: 20480,
        });
        expect(pass?.adapterElapsedMs).toBeCloseTo(18.3, 5);
    });

    it("ignores lines without the marker", () => {
        expect(parseRustPassLine("[eidnara] something else entirely")).toBeNull();
    });
});

describe("deleteMessagesAfter", () => {
    // The plugin orders session history by `(time_created, id)`, so a revert must remove a
    // later message that shares the anchor's millisecond, and must leave other sessions alone.
    it("removes every message after the anchor in (time_created, id) order within the session", () => {
        const db = new Database(":memory:");
        db.exec(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT)",
        );
        db.exec(
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT)",
        );
        const insert = db.prepare(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, '{}')",
        );
        insert.run("msg_a", "ses_1", 100);
        insert.run("msg_b", "ses_1", 200);
        insert.run("msg_c", "ses_1", 200);
        insert.run("msg_d", "ses_1", 300);
        insert.run("msg_z", "ses_2", 900);
        const part = db.prepare(
            "INSERT INTO part (id, message_id, session_id, data) VALUES (?, ?, ?, '{}')",
        );
        part.run("prt_b", "msg_b", "ses_1");
        part.run("prt_c", "msg_c", "ses_1");
        part.run("prt_z", "msg_z", "ses_2");

        expect(deleteMessagesAfter(db, "ses_1", "msg_b")).toBe(2);
        const remaining = db
            .prepare("SELECT id FROM message ORDER BY time_created, id")
            .all() as Array<{ id: string }>;
        expect(remaining.map((row) => row.id)).toEqual(["msg_a", "msg_b", "msg_z"]);
        const parts = db.prepare("SELECT id FROM part ORDER BY id").all() as Array<{ id: string }>;
        expect(parts.map((row) => row.id)).toEqual(["prt_b", "prt_z"]);
        db.close();
    });
});
