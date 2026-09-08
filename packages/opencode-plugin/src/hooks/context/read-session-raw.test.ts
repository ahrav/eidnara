import { describe, expect, it } from "bun:test";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    countRawSessionMessageOrdinalsFromDb,
    readRawSessionMessageByIdFromDb,
    readRawSessionMessageIdOrdinalsFromDb,
    readRawSessionMessageOrdinalByIdFromDb,
    readRawSessionMessagePageFromDb,
    readRawSessionMessagesFromDb,
} from "./read-session-raw";

describe("raw session message id ordinals", () => {
    it("matches the full raw reader across ordering, summaries, roles, and tool arcs", () => {
        const db = new Database(":memory:");
        try {
            db.exec(`
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
            `);
            const insertMessage = db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, 'session', ?, ?, ?)",
            );
            const rows: Array<[string, number, string]> = [
                ["m-tool-result", 30, JSON.stringify({ role: "tool", finish: "stop" })],
                [
                    "m-summary",
                    20,
                    JSON.stringify({ role: "assistant", summary: true, finish: "stop" }),
                ],
                ["m-weird", 20, JSON.stringify({ role: { unexpected: true }, summary: "true" })],
                ["m-user", 10, JSON.stringify({ role: "user" })],
                ["m-malformed", 25, "{"],
                // `JSON.parse` keeps the last duplicate key, so this row is a compaction summary in every reader.
                ["m-dup-key-summary", 25, '{"summary":false,"summary":true,"finish":"stop"}'],
                // A valid non-object document consumes an ordinal but has no addressable info, like a malformed row.
                ["m-array", 26, "[1]"],
                // Numeric `1` is not the JSON boolean `true`, so this row is an ordinary message in every reader.
                [
                    "m-numeric-summary",
                    27,
                    JSON.stringify({ role: "assistant", summary: 1, finish: "stop" }),
                ],
                [
                    "m-assistant",
                    20,
                    JSON.stringify({ role: "assistant", summary: true, finish: "tool-calls" }),
                ],
            ];
            for (const [id, createdAt, data] of rows) {
                insertMessage.run(id, createdAt, createdAt, data);
            }
            const insertPart = db.prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, 'session', ?, ?, ?)",
            );
            insertPart.run(
                "p-call",
                "m-assistant",
                21,
                21,
                JSON.stringify({ type: "tool", callID: "call-1", state: { status: "completed" } }),
            );
            insertPart.run(
                "p-result",
                "m-tool-result",
                31,
                31,
                JSON.stringify({ type: "tool_result", callID: "call-1", output: "done" }),
            );

            const fullReaderMap = new Map(
                readRawSessionMessagesFromDb(db, "session").map((message) => [
                    message.id,
                    message.ordinal,
                ]),
            );

            expect(readRawSessionMessageIdOrdinalsFromDb(db, "session")).toEqual(fullReaderMap);
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-user")).toBe(1);
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-assistant")).toBe(2);
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-numeric-summary")).toBe(
                6,
            );
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-summary")).toBeNull();
            expect(
                readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-dup-key-summary"),
            ).toBeNull();
            expect(readRawSessionMessageByIdFromDb(db, "session", "m-dup-key-summary")).toBeNull();
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "missing")).toBeNull();
            // Rows without a JSON-object info are not addressable by id in any reader, though they hold ordinals 4 and 5.
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-malformed")).toBeNull();
            expect(readRawSessionMessageOrdinalByIdFromDb(db, "session", "m-array")).toBeNull();
            expect(readRawSessionMessageByIdFromDb(db, "session", "m-malformed")).toBeNull();
            expect(readRawSessionMessageByIdFromDb(db, "session", "m-array")).toBeNull();
            expect([...fullReaderMap]).toEqual([
                ["m-user", 1],
                ["m-assistant", 2],
                ["m-weird", 3],
                ["m-numeric-summary", 6],
                ["m-tool-result", 7],
            ]);

            // The by-id reader counts ordinals across the earlier malformed row instead of aborting on it.
            expect(readRawSessionMessageByIdFromDb(db, "session", "m-tool-result")?.ordinal).toBe(
                7,
            );
            expect(
                readRawSessionMessageByIdFromDb(db, "session", "m-numeric-summary")?.ordinal,
            ).toBe(6);
            expect(readRawSessionMessageByIdFromDb(db, "session", "m-summary")).toBeNull();

            const firstPage = readRawSessionMessagePageFromDb(db, "session", 0, 2, 7);
            const secondPage = readRawSessionMessagePageFromDb(db, "session", 2, 5, 7);
            expect([...firstPage, ...secondPage].map(({ id, ordinal }) => [id, ordinal])).toEqual([
                ["m-user", 1],
                ["m-assistant", 2],
                ["m-weird", 3],
                ["m-malformed", 4],
                ["m-array", 5],
                ["m-numeric-summary", 6],
                ["m-tool-result", 7],
            ]);
            expect(countRawSessionMessageOrdinalsFromDb(db, "session")).toBe(7);

            // A negative cursor reads from the start, numbers the first row 1, and stays inside the watermark.
            expect(
                readRawSessionMessagePageFromDb(db, "session", -1, 100, 5).map(
                    ({ id, ordinal }) => [id, ordinal],
                ),
            ).toEqual([
                ["m-user", 1],
                ["m-assistant", 2],
                ["m-weird", 3],
                ["m-malformed", 4],
                ["m-array", 5],
            ]);
        } finally {
            closeQuietly(db);
        }
    });

    it("reads a page wider than one part-lookup chunk with every part attached in order", () => {
        const db = new Database(":memory:");
        try {
            db.exec(`
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
            `);
            const insertMessage = db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, 'session', ?, ?, ?)",
            );
            const insertPart = db.prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, 'session', ?, ?, ?)",
            );
            const messageCount = 1_700;
            db.exec("BEGIN");
            for (let i = 1; i <= messageCount; i += 1) {
                const id = `m-${String(i).padStart(5, "0")}`;
                insertMessage.run(id, i, i, JSON.stringify({ role: "user" }));
                insertPart.run(`${id}-b`, id, i * 10 + 2, i * 10 + 2, JSON.stringify({ seq: 2 }));
                insertPart.run(`${id}-a`, id, i * 10 + 1, i * 10 + 1, JSON.stringify({ seq: 1 }));
            }
            db.exec("COMMIT");

            const page = readRawSessionMessagePageFromDb(db, "session", 0, messageCount);
            expect(page.length).toBe(messageCount);
            expect(page[0]).toMatchObject({ id: "m-00001", ordinal: 1 });
            expect(page[messageCount - 1]).toMatchObject({
                id: `m-${String(messageCount).padStart(5, "0")}`,
                ordinal: messageCount,
            });
            for (const message of page) {
                expect(message.parts).toEqual([{ seq: 1 }, { seq: 2 }]);
            }
        } finally {
            closeQuietly(db);
        }
    });
});
