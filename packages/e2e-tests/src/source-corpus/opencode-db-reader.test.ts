import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { readOpenCodeSession } from "./opencode-db-reader.ts";

const SESSION = "ses_corpus";

function store(): Database {
    const db = new Database(":memory:");
    db.exec(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
    );
    db.exec(
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
    );
    return db;
}

function message(db: Database, id: string, at: number, data: object): void {
    db.prepare("INSERT INTO message VALUES (?, ?, ?, ?, ?)").run(
        id,
        SESSION,
        at,
        at,
        JSON.stringify(data),
    );
}

function parts(db: Database, messageId: string, items: object[]): void {
    const insert = db.prepare("INSERT INTO part VALUES (?, ?, ?, 0, 0, ?)");
    items.forEach((item, index) => {
        const id = `prt_${messageId}_${index.toString().padStart(2, "0")}`;
        insert.run(id, messageId, SESSION, JSON.stringify(item));
    });
}

const textPart = (body: string) => ({ type: "text", text: body });
const settledTool = (call: string, tool: string, metadata: object) => ({
    type: "tool",
    tool,
    callID: call,
    state: { status: "completed", input: {}, output: `${tool} ok`, metadata, time: { end: 2 } },
});

function partAt(db: Database, messageId: string, id: string, at: number, item: object): void {
    db.prepare("INSERT INTO part VALUES (?, ?, ?, ?, ?, ?)").run(
        id,
        messageId,
        SESSION,
        at,
        at,
        JSON.stringify(item),
    );
}

/** AE1: 12 text parts, 7 settled tool parts, 1 pending tool part, and 1 image part. */
function ae1Store(): Database {
    const db = store();
    message(db, "msg_1", 1, { role: "user", time: { created: 1 } });
    parts(db, "msg_1", [
        textPart("one"),
        textPart("two"),
        textPart("three"),
        { type: "file", mime: "image/png", url: "data:image/png;base64,AAAA" },
    ]);
    message(db, "msg_2", 2, { role: "assistant", time: { created: 2, completed: 3 } });
    parts(db, "msg_2", [
        { type: "step-start" },
        ...Array.from({ length: 9 }, (_, i) => textPart(`assistant ${i}`)),
        settledTool("c1", "bash", { exit: 0, truncated: false }),
        settledTool("c2", "read", {}),
        settledTool("c3", "edit", {}),
        settledTool("c4", "write", {}),
        settledTool("c5", "grep", {}),
        settledTool("c6", "glob", {}),
        {
            type: "tool",
            tool: "bash",
            callID: "c7",
            state: { status: "error", input: {}, error: "exit 1", metadata: { exit: 1 } },
        },
        { type: "tool", tool: "todowrite", callID: "c8", state: { status: "pending", input: {} } },
    ]);
    return db;
}

describe("OpenCode db reader", () => {
    it("reads 19 expected units and 2 classified non-units from the AE1 store", () => {
        const records = readOpenCodeSession(ae1Store(), SESSION);
        const units = records.flatMap((r) => r.units);
        const nonUnits = records.flatMap((r) => r.nonUnits);
        expect(units.length).toBe(19);
        expect(units.filter((u) => u.tool === null).length).toBe(12);
        expect(units.filter((u) => u.tool !== null).length).toBe(7);
        const classified = nonUnits.filter((n) => n.kind !== "step-start");
        expect(classified.map((n) => [n.reason, n.kind])).toEqual([
            ["non_text_part", "file"],
            ["unsettled_tool", "tool"],
        ]);
        expect(nonUnits.find((n) => n.kind === "step-start")?.reason).toBe("non_text_part");
        const bash = units.filter((u) => u.tool?.name === "bash");
        expect(bash.map((u) => [u.tool?.exit, u.tool?.isError, u.byteLen])).toEqual([
            [0, false, 7],
            [1, true, 6],
        ]);
        expect(bash[0]?.tool?.truncated).toBe(false);
        expect(units.find((u) => u.tool?.name === "read")?.tool?.exit).toBeNull();
        expect(records[0]?.units.map((u) => u.index)).toEqual([0, 1, 2]);
    });

    it("classifies ignored, unsettled, and interrupted parts, and counts UTF-8 bytes", () => {
        const db = store();
        message(db, "msg_1", 1, { role: "user", time: { created: 1 } });
        parts(db, "msg_1", [textPart("é"), { ...textPart("hidden"), ignored: true }]);
        message(db, "msg_2", 2, { role: "assistant", time: { created: 2 } });
        parts(db, "msg_2", [
            textPart("streaming"),
            settledTool("c1", "read", {}),
            {
                type: "tool",
                tool: "bash",
                callID: "c2",
                state: { status: "error", error: "aborted", metadata: { interrupted: true } },
            },
            { type: "reasoning", text: "thinking" },
        ]);
        const [user, assistant] = readOpenCodeSession(db, SESSION);
        expect(user?.units.map((u) => u.byteLen)).toEqual([2]);
        expect(user?.nonUnits.map((n) => n.reason)).toEqual(["ignored_text"]);
        expect(assistant?.units.map((u) => u.tool?.name)).toEqual(["read"]);
        expect(assistant?.nonUnits.map((n) => [n.index, n.reason])).toEqual([
            [0, "unsettled_text"],
            [2, "unsettled_tool"],
            [3, "non_text_part"],
        ]);
    });

    it("refuses a part whose settled output is not a string", () => {
        const db = store();
        message(db, "msg_1", 1, { role: "assistant", time: { created: 1, completed: 2 } });
        parts(db, "msg_1", [
            { type: "tool", tool: "read", callID: "c", state: { status: "completed", output: 1 } },
        ]);
        expect(() => readOpenCodeSession(db, SESSION)).toThrow(/output: not a string/);
    });

    it("indexes parts by (time_created, id), the order the harness presents them in", () => {
        const db = store();
        message(db, "msg_1", 1, { role: "user", time: { created: 1 } });
        partAt(db, "msg_1", "prt_z", 10, textPart("first"));
        partAt(db, "msg_1", "prt_b", 20, textPart("second"));
        partAt(db, "msg_1", "prt_a", 20, textPart("third"));
        const [user] = readOpenCodeSession(db, SESSION);
        expect(user?.units.map((u) => [u.index, u.partId, u.byteLen])).toEqual([
            [0, "prt_z", 5],
            [1, "prt_a", 5],
            [2, "prt_b", 6],
        ]);
    });

    it("leaves compaction summary messages out of the expected table", () => {
        const db = store();
        message(db, "msg_1", 1, { role: "user", time: { created: 1 } });
        parts(db, "msg_1", [textPart("ask")]);
        message(db, "msg_2", 2, {
            role: "assistant",
            summary: true,
            finish: "stop",
            time: { created: 2, completed: 3 },
        });
        parts(db, "msg_2", [textPart("compacted history")]);
        message(db, "msg_3", 3, {
            role: "assistant",
            summary: true,
            time: { created: 3 },
        });
        parts(db, "msg_3", [textPart("streaming summary")]);
        expect(readOpenCodeSession(db, SESSION).map((r) => r.id)).toEqual(["msg_1", "msg_3"]);
    });
});
