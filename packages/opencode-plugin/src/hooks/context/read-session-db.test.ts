import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    closeReadOnlySessionDb,
    findLastAssistantModelFromOpenCodeDb,
    isMidTurn,
    isMidTurnFromOpenCodeDb,
} from "./read-session-db";

const tempDirs: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

afterEach(() => {
    // Close the cached OpenCode read-only DB handle so the next test case opens a DB under the new XDG_DATA_HOME.
    closeReadOnlySessionDb();
    process.env.XDG_DATA_HOME = originalXdgDataHome;
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
    // An idle assistant row: finished with "stop" and stamped with time.completed.
    function insertIdleAssistant(db: Database, timeCreated: number): void {
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" }, timeCreated);
        insertPart(db, "session-1", "assistant-1", "assistant-part-1", {
            type: "text",
            text: "done",
        });
    }

    it("is not mid-turn when there is no message at all", () => {
        const db = createMidTurnDb();

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
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

    it("is mid-turn while the latest assistant has no parts yet", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { time: { created: 100 } }, 100);

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

    it("is mid-turn when the latest assistant has a non-provider-executed tool part", () => {
        const db = createMidTurnDb();
        insertAssistant(db, "session-1", "assistant-1", { finish: "stop" }, 100);
        insertPart(db, "session-1", "assistant-1", "part-1", {
            type: "tool",
            providerExecuted: false,
        });

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

    it("is not mid-turn when the latest assistant has no tool parts", () => {
        const db = createMidTurnDb();
        insertIdleAssistant(db, 100);

        expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
    });

    // A real user message newer than the latest assistant row is a turn whose
    // assistant row has not been created yet, so the session reads as mid-turn
    // even when that latest assistant is idle.
    describe("pending turn: real user message newer than the latest assistant", () => {
        it("is mid-turn for a partless user message after an idle assistant", () => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
            insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
        });

        it("is mid-turn for a real user message after a stale tool-calls tail", () => {
            const db = createMidTurnDb();
            insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
            insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
        });

        it("is mid-turn for a real user message after an unfinished assistant", () => {
            const db = createMidTurnDb();
            insertAssistant(db, "session-1", "assistant-1", { time: { created: 100 } }, 100);
            insertUser(db, "session-1", "user-1", { content: "new turn" }, 200);

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
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

        it("is mid-turn for an @mention operator prompt with a synthetic agent part", () => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
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

        it("is mid-turn when a user message has a marker part AND a real text part", () => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
            insertUser(db, "session-1", "user-1", { content: "real input with marker" }, 200);
            insertPart(db, "session-1", "user-1", "part-1", {
                type: "text",
                text: "\u2709 Inbox from peer",
                metadata: { marker: { kind: "inbox" } },
            });
            insertPart(db, "session-1", "user-1", "part-2", {
                type: "text",
                text: "real input with marker",
            });

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
        });

        it("is mid-turn when a user message has an ignored part AND a real text part", () => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
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

        it("is mid-turn for real text with a file attachment part", () => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
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
            insertIdleAssistant(db, 100);
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
            insertIdleAssistant(db, 100);
            insertUser(db, "session-1", "user-1", { content: "continue with the fix" }, 200);
            insertPart(db, "session-1", "user-1", "part-1", { type: "step-start" });
            insertPart(db, "session-1", "user-1", "part-2", {
                type: "text",
                text: "continue with the fix",
            });
            insertPart(db, "session-1", "user-1", "part-3", { type: "step-finish" });

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
        });
    });

    // Machine-authored user messages do not start a turn. After an idle
    // assistant they leave the session idle; after a stale tool-calls tail
    // they leave it mid-turn because the tail still governs.
    describe("machine-authored user messages are not a pending turn", () => {
        const machineParts: Array<[string, string, Record<string, unknown>]> = [
            [
                "synthetic text part",
                "agent nudge",
                { type: "text", text: "agent nudge", synthetic: true },
            ],
            [
                "inbox marker part",
                "\u2709 Inbox from peer",
                {
                    type: "text",
                    text: "\u2709 Inbox from peer",
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
                "ignored-only part",
                "status notification",
                { type: "text", text: "## Claude Routing Status", ignored: true },
            ],
            [
                "ignored numeric 1 part",
                "status notification",
                { type: "text", text: "## Claude Quotas", ignored: 1 },
            ],
            [
                "interrupt marker part",
                "interrupt",
                {
                    type: "text",
                    text: "interrupt",
                    metadata: { marker: { kind: "interrupt", intent: "abort", origin: "parent" } },
                },
            ],
            [
                "message marker part",
                "peer message",
                {
                    type: "text",
                    text: "peer message",
                    metadata: { marker: { kind: "message", peer: "subagent", expectReply: false } },
                },
            ],
        ];

        it.each(
            machineParts,
        )("%s after an idle assistant leaves the session idle", (_title, content, part) => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
            insertUser(db, "session-1", "user-1", { content }, 200);
            insertPart(db, "session-1", "user-1", "part-1", part);

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
        });

        it.each(
            machineParts,
        )("%s after a stale tool-calls tail leaves the session mid-turn", (_title, content, part) => {
            const db = createMidTurnDb();
            insertAssistant(db, "session-1", "assistant-1", { finish: "tool-calls" }, 100);
            insertUser(db, "session-1", "user-1", { content }, 200);
            insertPart(db, "session-1", "user-1", "part-1", part);

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(true);
        });

        it("leaves the session idle when every part is synthetic, including a patch part", () => {
            const db = createMidTurnDb();
            insertIdleAssistant(db, 100);
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

            expect(isMidTurnFromOpenCodeDb(db, "session-1")).toBe(false);
        });
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
        for (const row of rows) {
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
    } finally {
        closeQuietly(db);
    }
}

describe("isMidTurn", () => {
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
});

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
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
        });
        expect(findLastAssistantModelFromOpenCodeDb("ses_B")).toEqual({
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
            providerID: "anthropic",
            modelID: "claude-opus-4-7",
        });
        // `agent` must be absent; an empty string triggers the `agentBySession` lookup.
        // `recovered.agent` must be absent; an empty string triggers the `agentBySession` lookup.
        // `recovered.agent` must be absent; an empty string triggers the `agentBySession` lookup.
        expect((result as { agent?: string }).agent).toBeUndefined();
    });
});
