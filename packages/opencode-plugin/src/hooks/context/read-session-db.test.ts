import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    closeReadOnlySessionDb,
    findLastAssistantModelFromOpenCodeDb,
    getMessageTimesFromOpenCodeDb,
    isMidTurn,
    isMidTurnFromOpenCodeDb,
    openCodeDbExists,
} from "./read-session-db";

const tempDirs: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;
const originalOpenCodeDb = process.env.OPENCODE_DB;

afterEach(() => {
    // Close the cached OpenCode read-only DB handle so the next test case opens a DB under the new XDG_DATA_HOME.
    closeReadOnlySessionDb();
    process.env.XDG_DATA_HOME = originalXdgDataHome;
    if (originalOpenCodeDb === undefined) {
        delete process.env.OPENCODE_DB;
    } else {
        process.env.OPENCODE_DB = originalOpenCodeDb;
    }
    for (const dir of tempDirs) {
        try {
            rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* */
        }
    }
    tempDirs.length = 0;
});

function createMidTurnDb(): Database {
    const db = new Database(":memory:");
    db.exec(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    db.exec(
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    return db;
}

// A finished assistant row carries `time.completed`; pass `time: { created }` for a message still being produced.
function insertAssistant(
    db: Database,
    sessionId: string,
    id: string,
    data: Record<string, unknown>,
    timeCreated = Date.now(),
): void {
    db.prepare(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
    ).run(
        id,
        sessionId,
        timeCreated,
        timeCreated,
        JSON.stringify({
            role: "assistant",
            time: { created: timeCreated, completed: timeCreated },
            ...data,
        }),
    );
}

function insertUser(
    db: Database,
    sessionId: string,
    id: string,
    data: Record<string, unknown>,
    timeCreated: number,
): void {
    db.prepare(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
    ).run(id, sessionId, timeCreated, timeCreated, JSON.stringify({ role: "user", ...data }));
}

function insertPart(
    db: Database,
    sessionId: string,
    messageId: string,
    id: string,
    data: unknown,
): void {
    db.prepare(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?)",
    ).run(id, messageId, sessionId, Date.now(), Date.now(), JSON.stringify(data));
}

describe("isMidTurnFromOpenCodeDb", () => {
    it("is not mid-turn when there is no message at all", () => {
        const db = createMidTurnDb();

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is mid-turn for the first real user message before any assistant row exists", () => {
        const db = createMidTurnDb();
        insertUser(db, "session-1", "user-1", { content: "first prompt" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "text", text: "first prompt" });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is not mid-turn for an ignored-only user message before any assistant row exists", () => {
        const db = createMidTurnDb();
        insertUser(db, "session-1", "user-1", { content: "status" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "## Claude Quotas",
            ignored: true,
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is mid-turn while the latest assistant has no parts yet", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { time: { created: 100 } }, 100);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn while the latest assistant is still streaming text", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { time: { created: 100 } }, 100);
        insertPart(db, "session-1", "assistant-1", "part-1", { type: "step-start" });
        insertPart(db, "session-1", "assistant-1", "part-2", {
            type: "text",
            text: "partial answer",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a real user message after an unfinished assistant", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { time: { created: 100 } }, 100);
        insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is not mid-turn for an aborted assistant that completed without a finish reason", () => {
        const db = createMidTurnDb();
        insertAssistant(
            db,
            "session-1",
            "assistant-1",
            { error: { name: "MessageAbortedError" }, time: { created: 100, completed: 150 } },
            100,
        );
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "text",
            text: "partial answer",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is mid-turn when the latest assistant finished with tool-calls", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a real user message after a stale tool-calls tail", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    // Machine-authored single-part user messages must not release a stale
    // tool-calls tail: each row inserts one user part variant after the stale
    // assistant and asserts the session still reads as mid-turn.
    it.each([
        [
            "does not release mid-turn for synthetic-part user messages after a stale tool-calls tail",
            "agent nudge",
            { type: "text", text: "agent nudge", synthetic: true },
        ],
        [
            "does not release mid-turn for marker-part user messages after a stale tool-calls tail",
            "✉ Inbox from peer",
            {
                type: "text",
                text: "✉ Inbox from peer",
                metadata: {
                    marker: {
                        kind: "inbox",
                        from: "Peer Session",
                        sessionId: "ses_peer0000000000000000000",
                    },
                },
            },
        ],
        [
            "does not release mid-turn for an ignored-only user part after a stale tool-calls tail",
            "status notification",
            { type: "text", text: "## Claude Routing Status", ignored: true },
        ],
        [
            "does not release mid-turn when ignored is numeric 1 (truthy variant)",
            "status notification",
            { type: "text", text: "## Claude Quotas", ignored: 1 },
        ],
        [
            "does not release mid-turn for interrupt marker parts after a stale tool-calls tail",
            "interrupt",
            {
                type: "text",
                text: "interrupt",
                metadata: { marker: { kind: "interrupt", intent: "abort", origin: "parent" } },
            },
        ],
        [
            "does not release mid-turn for message marker parts after a stale tool-calls tail",
            "peer message",
            {
                type: "text",
                text: "peer message",
                metadata: { marker: { kind: "message", peer: "subagent", expectReply: false } },
            },
        ],
    ] as Array<[string, string, Record<string, unknown>]>)("%s", (_title, content, part) => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content }, 200);
        insertPart(db, "session-1", "user-1", "part-1", part);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn when the latest assistant has a non-provider-executed tool part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" }, 100);
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            providerExecuted: false,
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a real user message after an unexecuted tool tail", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" }, 100);
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            providerExecuted: false,
        });
        insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is not mid-turn for provider-executed tool parts", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" });
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            providerExecuted: true,
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is not mid-turn when the provider-executed flag sits under metadata (persisted OpenCode shape)", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" });
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            tool: "web_search",
            callID: "call-1",
            state: { status: "completed", input: {} },
            metadata: { providerExecuted: true },
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("stays mid-turn when metadata is present but providerExecuted is not set", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" });
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            tool: "todowrite",
            state: { status: "completed", input: {} },
            metadata: { openai: { itemId: "fc_1" } },
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is not mid-turn when the only tool part is the daemon's synthetic todo marker", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" });
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            tool: "todowrite",
            syntheticTodoMarker: true,
            state: { status: "completed", input: {}, output: "[]" },
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is not mid-turn when the latest assistant has no tool parts", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" });
        insertPart(db, "session-1", "assistant-1", "part-1", { type: "text", text: "done" });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is mid-turn for an @mention operator prompt with a synthetic agent part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "do the thing @research-deep" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "do the thing @research-deep",
        });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "agent",
            name: "research-deep",
            synthetic: true,
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a partless user message after an idle assistant", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);
        // Partless messages count as real.

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn when a user message has a marker part AND a real text part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "real input with marker" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "✉ Inbox from peer",
            metadata: { marker: { kind: "inbox" } },
        });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "text",
            text: "real input with marker",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for real text with a file attachment part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "review this file" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "review this file",
        });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "file",
            mime: "text/plain",
            url: "file:///tmp/example.txt",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a file-only user message without machine markers", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "file",
            mime: "image/png",
            url: "data:image/png;base64,AAAA",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn when step boundary parts accompany real text", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "continue with the fix" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "step-start" });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "text",
            text: "continue with the fix",
        });
        insertPart(db, "session-1", "user-1", "part-3", { type: "step-finish" });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("does not release when every part is synthetic, including a patch part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "generated update" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "generated update",
            synthetic: true,
        });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "patch",
            hash: "abc123",
            files: ["src/example.ts"],
            synthetic: true,
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    // Unflagged machine-authored text must not release a mid-turn session.
    it.each([
        ["a system reminder", "<system-reminder>ignore</system-reminder>"],
        [
            "a nested system reminder",
            "<system-reminder>a <system-reminder>b</system-reminder> c</system-reminder>",
        ],
        ["the initiator marker", "<!-- OMO_INTERNAL_INITIATOR -->"],
        ["a system directive", "[SYSTEM DIRECTIVE: EIDNARA do the thing]"],
        ["whitespace", "   \n  "],
        [
            "a reminder wrapping a directive",
            "<system-reminder>x</system-reminder> [SYSTEM DIRECTIVE: EIDNARA y]",
        ],
    ])("does not release mid-turn for an unflagged user part that is only %s", (_label, text) => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: text }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "text", text });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for real text that sits beside an unflagged system reminder in the same part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "<system-reminder>ignore</system-reminder> please continue",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("does not release for a text part whose text is not a string", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "text", text: 42 });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it.each([
        ["a JSON array", []],
        ["an object without a type", { foo: 1 }],
        ["an object with a non-string type", { type: 7 }],
        ["a JSON string", "text"],
    ])("does not release for a part that is %s", (_label, data) => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", data);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("does not release for an Oh My OpenCode directive part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "[SYSTEM DIRECTIVE: OH-MY-OPENCODE continue]",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it.each([
        "[task CALL FAILED - IMMEDIATE RETRY REQUIRED] retry now",
        "[Category+Skill Reminder] remember the skill",
        "Unstable background agent appears idle",
        "[EMERGENCY CONTEXT WINDOW WARNING] compact",
        "§42§ [SYSTEM DIRECTIVE: EIDNARA continue]",
        "§42§ <system-reminder>hidden</system-reminder>",
        "<system-reminder>x</system-reminder> §42§ [SYSTEM DIRECTIVE: EIDNARA y]",
    ])("does not release for the unflagged machine notice %j", (notice) => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: notice }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "text", text: notice });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for authored text that shares a part with an embedded notice", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "please continue\n\nUnstable background agent appears idle\ndetails",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is not mid-turn when there is no assistant message", () => {
        const db = createMidTurnDb();

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is mid-turn when a user message has an ignored part AND a real text part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "notification + real input" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", {
            type: "text",
            text: "## Claude Quotas",
            ignored: true,
        });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "text",
            text: "actually do the thing",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a real user message that shares the assistant's millisecond with a later id", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "msg_a", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "msg_b", { content: "new turn" }, 100);
        insertPart(db, "session-1", "msg_b", "part-1", { type: "text", text: "new turn" });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("does not release for a user message that shares the assistant's millisecond with an earlier id", () => {
        const db = createMidTurnDb();
        insertUser(db, "session-1", "msg_a", { content: "earlier turn" }, 100);
        insertPart(db, "session-1", "msg_a", "part-1", { type: "text", text: "earlier turn" });
        insertAssistant(db, "session-1", "msg_b", { finish: "tool-calls" }, 100);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("picks the later id when two assistant rows share a millisecond", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "msg_a1", { finish: "tool-calls" }, 100);
        insertAssistant(db, "session-1", "msg_a2", { finish: "stop" }, 100);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    // `compaction` identifies a machine-authored message even when an adjacent text part is unflagged.
    it.each([
        ["auto", true],
        ["manual", false],
    ])("does not release mid-turn for an %s compaction user message", (_kind, auto) => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "compaction", auto });
        insertPart(db, "session-1", "user-1", "part-2", {
            type: "text",
            text: "Summarize the conversation so far.",
        });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a real user turn that follows a compaction message", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "compaction", auto: true });
        insertPart(db, "session-1", "user-1", "part-2", { type: "text", text: "Summarize." });
        insertUser(db, "session-1", "user-2", { content: "next task" }, 300);
        insertPart(db, "session-1", "user-2", "part-3", { type: "text", text: "next task" });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("stays mid-turn when a compaction summary assistant follows the tool-calls assistant", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "compaction", auto: true });
        insertPart(db, "session-1", "user-1", "part-2", { type: "text", text: "Summarize." });
        insertAssistant(db, "session-1", "assistant-2", { summary: true, finish: "stop" }, 300);
        insertPart(db, "session-1", "assistant-2", "part-3", { type: "text", text: "Summary." });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is not mid-turn once the post-compaction continuation finishes with stop", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        insertPart(db, "session-1", "user-1", "part-1", { type: "compaction", auto: true });
        insertAssistant(db, "session-1", "assistant-2", { summary: true, finish: "stop" }, 300);
        insertAssistant(db, "session-1", "assistant-3", { finish: "stop" }, 400);
        insertPart(db, "session-1", "assistant-3", "part-2", { type: "text", text: "Done." });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("is not mid-turn when the only assistant row is a compaction summary", () => {
        const db = createMidTurnDb();
        insertUser(db, "session-1", "user-1", { content: "" }, 100);
        insertPart(db, "session-1", "user-1", "part-1", { type: "compaction", auto: false });
        insertAssistant(db, "session-1", "assistant-1", { summary: true, finish: "stop" }, 200);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    it("stays mid-turn when an older message row holds malformed JSON", () => {
        const db = createMidTurnDb();
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        ).run("broken", "session-1", 10, 10, "{");
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("treats a malformed part as no evidence of a real user turn", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "" }, 200);
        db.prepare(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?)",
        ).run("part-broken", "user-1", "session-1", 0, 0, "{");

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });

    it("is mid-turn for a real part that sits beside a malformed part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
        insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);
        db.prepare(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?)",
        ).run("part-broken", "user-1", "session-1", 0, 0, "{");
        insertPart(db, "session-1", "user-1", "part-2", { type: "text", text: "new turn" });

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
    });
});

function useTempDataHome(prefix: string): void {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
}

interface MessageRow {
    id: string;
    sessionId: string;
    role: "user" | "assistant";
    providerID?: string;
    modelID?: string;
    agent?: string;
    timeCreated: number;
}

function createOpenCodeDb(rows: MessageRow[]): void {
    const dbPath = join(process.env.XDG_DATA_HOME!, "opencode", "opencode.db");
    mkdirSync(dirname(dbPath), { recursive: true });
    const db = new Database(dbPath);
    try {
        db.exec(`
            CREATE TABLE IF NOT EXISTS message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
        `);
        const insert = db.prepare(
            `INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?, ?, ?, ?, ?)`,
        );
        // A single transaction avoids an fsync for each inserted row.
        db.transaction(() => {
            for (const row of rows) {
                // Every fixture row is a finished message, so it carries `time.completed`.
                const data: Record<string, unknown> = {
                    role: row.role,
                    time: { created: row.timeCreated, completed: row.timeCreated },
                };
                if (row.providerID !== undefined) data.providerID = row.providerID;
                if (row.modelID !== undefined) data.modelID = row.modelID;
                if (row.agent !== undefined) data.agent = row.agent;
                insert.run(
                    row.id,
                    row.sessionId,
                    row.timeCreated,
                    row.timeCreated,
                    JSON.stringify(data),
                );
            }
        })();
    } finally {
        closeQuietly(db);
    }
}

describe("findLastAssistantModelFromOpenCodeDb", () => {
    it("returns null for a session with no assistant messages", () => {
        useTempDataHome("read-session-db-no-assistant-");
        createOpenCodeDb([
            {
                id: "msg_user1",
                sessionId: "ses_A",
                role: "user",
                timeCreated: 1000,
            },
        ]);
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toBeNull();
    });

    it("returns the most recent assistant's providerID/modelID", () => {
        useTempDataHome("read-session-db-latest-assistant-");
        createOpenCodeDb([
            {
                id: "msg_old",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-sonnet-4.5",
                timeCreated: 1000,
            },
            {
                id: "msg_new",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-opus-4-7",
                timeCreated: 2000,
            },
        ]);
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_new",
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
        });
    });

    it("ignores user messages even when they are newer", () => {
        useTempDataHome("read-session-db-ignore-user-");
        createOpenCodeDb([
            {
                id: "msg_asst",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "github-copilot",
                modelID: "claude-sonnet-4.5",
                timeCreated: 1000,
            },
            {
                id: "msg_user_newer",
                sessionId: "ses_A",
                role: "user",
                timeCreated: 2000,
            },
        ]);
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_asst",
            providerID: "github-copilot",
            modelID: "claude-sonnet-4.5",
        });
    });

    it("ignores assistants without providerID or modelID", () => {
        useTempDataHome("read-session-db-incomplete-assistant-");
        createOpenCodeDb([
            {
                id: "msg_full",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-opus-4-7",
                timeCreated: 1000,
            },
            {
                id: "msg_missing_model",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                // modelID missing
                timeCreated: 2000,
            },
        ]);
        // `findLastAssistantModelFromOpenCodeDb` returns the fully populated earlier assistant rather than the newer partial row.
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_full",
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
        });
    });

    it("scopes by session ID and does not leak across sessions", () => {
        useTempDataHome("read-session-db-session-scope-");
        createOpenCodeDb([
            {
                id: "msg_A1",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-opus-4-7",
                timeCreated: 1000,
            },
            {
                id: "msg_B1",
                sessionId: "ses_B",
                role: "assistant",
                providerID: "github-copilot",
                modelID: "gpt-5.4",
                timeCreated: 2000,
            },
        ]);
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_A1",
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
        });
        expect(findLastAssistantModelFromOpenCodeDb("ses_B")).toEqual({
            messageID: "msg_B1",
            providerID: "github-copilot",
            modelID: "gpt-5.4",
        });
    });

    it("returns null gracefully when the DB is missing entirely", () => {
        useTempDataHome("read-session-db-missing-db-");
        // `findLastAssistantModelFromOpenCodeDb` logs an absent session DB error and returns `null` without creating a DB.
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toBeNull();
    });

    it("includes agent name when present on the assistant message", () => {
        useTempDataHome("read-session-db-agent-");
        createOpenCodeDb([
            {
                id: "msg_agentic",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-opus-4-7",
                agent: "Alfonso - CTO",
                timeCreated: 1000,
            },
        ]);
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_agentic",
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
            agent: "Alfonso - CTO",
        });
    });

    it("omits agent when missing or empty on the assistant message", () => {
        useTempDataHome("read-session-db-no-agent-");
        createOpenCodeDb([
            {
                id: "msg_default",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-opus-4-7",
                // no agent
                timeCreated: 1000,
            },
        ]);
        const result = findLastAssistantModelFromOpenCodeDb("ses_A");
        expect(result).toEqual({
            messageID: "msg_default",
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
        });
        // `agent` must be absent; an empty string triggers the `agentBySession` lookup.
        expect((result as { agent?: string }).agent).toBeUndefined();
    });
});

describe("isMidTurn", () => {
    it("reports idle for a readable DB with no active run", () => {
        useTempDataHome("read-session-db-midturn-idle-");
        createOpenCodeDb([
            {
                id: "msg_asst",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "anthropic",
                modelID: "claude-opus-4-7",
                timeCreated: 1000,
            },
        ]);

        expect(isMidTurn(undefined, "ses_A")).toBe(false);
    });

    it("reports idle when the OpenCode DB does not exist", () => {
        useTempDataHome("read-session-db-midturn-missing-");

        expect(isMidTurn(undefined, "ses_A")).toBe(false);
    });

    it("reports mid-turn when the OpenCode DB exists but cannot be read", () => {
        useTempDataHome("read-session-db-midturn-unreadable-");
        const dbPath = join(process.env.XDG_DATA_HOME!, "opencode", "opencode.db");
        mkdirSync(dirname(dbPath), { recursive: true });
        writeFileSync(dbPath, "not a sqlite database");

        expect(isMidTurn(undefined, "ses_A")).toBe(true);
    });
});

describe("OPENCODE_DB override", () => {
    it("reads the database OpenCode was pointed at instead of the XDG default", () => {
        useTempDataHome("read-session-db-xdg-");
        createOpenCodeDb([
            {
                id: "msg_xdg",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "xdg-provider",
                modelID: "xdg-model",
                timeCreated: 100,
            },
        ]);

        const overrideDir = mkdtempSync(join(tmpdir(), "read-session-db-override-"));
        tempDirs.push(overrideDir);
        const overridePath = join(overrideDir, "elsewhere.db");
        const db = new Database(overridePath);
        try {
            db.exec(
                "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
            );
            db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
            ).run(
                "msg_override",
                "ses_A",
                200,
                200,
                JSON.stringify({
                    role: "assistant",
                    providerID: "override-provider",
                    modelID: "override-model",
                }),
            );
        } finally {
            closeQuietly(db);
        }

        process.env.OPENCODE_DB = overridePath;
        expect(openCodeDbExists()).toBe(true);
        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_override",
            providerID: "override-provider",
            modelID: "override-model",
        });
    });

    it("ignores an empty OPENCODE_DB", () => {
        useTempDataHome("read-session-db-empty-override-");
        createOpenCodeDb([
            {
                id: "msg_xdg",
                sessionId: "ses_A",
                role: "assistant",
                providerID: "xdg-provider",
                modelID: "xdg-model",
                timeCreated: 100,
            },
        ]);
        process.env.OPENCODE_DB = "";

        expect(findLastAssistantModelFromOpenCodeDb("ses_A")).toEqual({
            messageID: "msg_xdg",
            providerID: "xdg-provider",
            modelID: "xdg-model",
        });
    });
});

describe("getMessageTimesFromOpenCodeDb", () => {
    it("returns an empty map for no ids without touching the DB", () => {
        expect(getMessageTimesFromOpenCodeDb("ses_A", [])).toEqual(new Map());
    });

    it("resolves times for an id list far larger than one IN clause", () => {
        useTempDataHome("read-session-db-message-times-");
        const ids = Array.from({ length: 2_500 }, (_, i) => `msg_${i}`);
        createOpenCodeDb(
            ids.map((id, i) => ({ id, sessionId: "ses_A", role: "user" as const, timeCreated: i })),
        );

        const times = getMessageTimesFromOpenCodeDb("ses_A", ids);

        expect(times.size).toBe(ids.length);
        expect(times.get("msg_0")).toBe(0);
        expect(times.get("msg_2499")).toBe(2499);
    });

    it("scopes by session and skips unknown ids", () => {
        useTempDataHome("read-session-db-message-times-scope-");
        createOpenCodeDb([
            { id: "msg_a", sessionId: "ses_A", role: "user", timeCreated: 10 },
            { id: "msg_b", sessionId: "ses_B", role: "user", timeCreated: 20 },
        ]);

        expect(getMessageTimesFromOpenCodeDb("ses_A", ["msg_a", "msg_b", "msg_missing"])).toEqual(
            new Map([["msg_a", 10]]),
        );
    });
});
