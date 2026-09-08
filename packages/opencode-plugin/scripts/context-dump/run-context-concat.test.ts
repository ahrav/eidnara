import { Database } from "bun:sqlite";
import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import Tokenizer from "ai-tokenizer";
import * as claude from "ai-tokenizer/encoding/claude";
import {
    concatSessionMessages,
    formatAnchor,
    indexAfterAnchor,
    parseAnchor,
    runContextConcat,
} from "./run-context-concat";
import type { DumpMessage } from "./types";

const tokenizer = new Tokenizer(claude);

function text(role: string, body: string): DumpMessage {
    return { info: { role }, parts: [{ type: "text", text: body }] };
}

function toolsOnly(count: number): DumpMessage {
    const parts = Array.from({ length: count }, () => ({ type: "tool" }));
    return { info: { role: "assistant" }, parts };
}

function empty(role: string): DumpMessage {
    return { info: { role }, parts: [] };
}

describe("concatSessionMessages", () => {
    test("totalTokens is the count of the joined output and never exceeds the budget", () => {
        const messages = Array.from({ length: 40 }, (_, i) =>
            text(i % 2 === 0 ? "user" : "assistant", `message ${i} lorem ipsum dolor sit amet`),
        );
        const renderedLines = messages.map(
            (_, i) =>
                `[${i}] ${i % 2 === 0 ? "User" : "Assistant"}: message ${i} lorem ipsum dolor sit amet`,
        );
        const perLineSum = renderedLines.reduce((sum, line) => sum + tokenizer.count(line), 0);
        const joinedAll = tokenizer.count(renderedLines.join("\n"));
        // Separators make the joined count larger than the sum.
        expect(joinedAll).toBeGreaterThan(perLineSum);

        for (const budget of [perLineSum - 5, perLineSum, perLineSum + 5, joinedAll]) {
            const page = concatSessionMessages(messages, budget);
            expect(page.totalTokens).toBe(tokenizer.count(page.output));
            expect(page.totalTokens).toBeLessThanOrEqual(budget);
        }

        const full = concatSessionMessages(messages, joinedAll);
        expect(full.messagesWithContent).toBe(messages.length);
        expect(full.hasMore).toBe(false);
    });

    test("a rejected mid-page tool summary does not advance endIndex past the tool run", () => {
        const messages = [
            text("user", "first question about the code"),
            toolsOnly(2),
            toolsOnly(1),
            text("assistant", "here is the answer"),
        ];
        const firstLine = "[0] User: first question about the code";
        // Budget admits the first line but not the tool summary that follows it.
        const budget = tokenizer.count(firstLine);

        const page = concatSessionMessages(messages, budget);
        expect(page.output).toBe(firstLine);
        expect(page.endIndex).toBe(0);
        expect(page.messagesWithContent).toBe(1);
        expect(page.hasMore).toBe(true);

        // Resuming at endIndex + 1 re-reads the tool run instead of skipping it.
        const next = concatSessionMessages(messages, 1000, page.endIndex + 1);
        expect(next.output).toBe("[1] Assistant: 3 tool calls\n[3] Assistant: here is the answer");
        expect(next.messagesWithContent).toBe(3);
        expect(next.hasMore).toBe(false);
    });

    test("a rejected trailing tool summary leaves hasMore true", () => {
        const messages = [text("user", "first question about the code"), toolsOnly(2)];
        const firstLine = "[0] User: first question about the code";
        const budget = tokenizer.count(firstLine);

        const page = concatSessionMessages(messages, budget);
        expect(page.output).toBe(firstLine);
        expect(page.endIndex).toBe(0);
        expect(page.hasMore).toBe(true);
    });

    test("an admitted tool run counts its messages and advances endIndex to its last message", () => {
        const messages = [toolsOnly(1), toolsOnly(2), text("user", "next")];
        const page = concatSessionMessages(messages, 1000);
        expect(page.output).toBe("[0] Assistant: 3 tool calls\n[2] User: next");
        expect(page.messagesWithContent).toBe(3);
        expect(page.endIndex).toBe(2);
        expect(page.hasMore).toBe(false);
    });

    test("messages without text or tool parts are consumed so hasMore reaches false", () => {
        const messages = [text("user", "hello"), empty("user"), empty("system")];
        const page = concatSessionMessages(messages, 1000);
        expect(page.output).toBe("[0] User: hello");
        expect(page.endIndex).toBe(2);
        expect(page.hasMore).toBe(false);
    });

    test("a trailing assistant message without time.completed is left unconsumed", () => {
        const inProgress: DumpMessage = {
            info: { role: "assistant", time: { created: 1 } },
            parts: [],
        };
        const messages = [text("user", "hello"), inProgress];
        const page = concatSessionMessages(messages, 1000);
        expect(page.output).toBe("[0] User: hello");
        expect(page.endIndex).toBe(0);
        expect(page.hasMore).toBe(true);

        // Partial text and streamed tool calls are held too; only completion releases the message.
        for (const parts of [
            [{ type: "text", text: "partial" }],
            [{ type: "tool" }],
            [{ type: "tool" }, { type: "text", text: "partial" }],
        ]) {
            const streaming: DumpMessage = {
                info: { role: "assistant", time: { created: 1 } },
                parts,
            };
            const held = concatSessionMessages([text("user", "hello"), streaming], 1000);
            expect(held.output).toBe("[0] User: hello");
            expect(held.endIndex).toBe(0);
            expect(held.hasMore).toBe(true);
        }

        // Resuming at the unfinished message reports no progress rather than skipping it.
        const retry = concatSessionMessages(messages, 1000, page.endIndex + 1);
        expect(retry.output).toBe("");
        expect(retry.endIndex).toBe(0);
        expect(retry.hasMore).toBe(true);

        const finished: DumpMessage = {
            info: { role: "assistant", time: { created: 1, completed: 2 } },
            parts: [{ type: "text", text: "done" }],
        };
        const after = concatSessionMessages([messages[0] as DumpMessage, finished], 1000, 1);
        expect(after.output).toBe("[1] Assistant: done");
        expect(after.endIndex).toBe(1);
        expect(after.hasMore).toBe(false);
    });

    test("a trailing empty assistant message that completed is consumed", () => {
        const completedEmpty: DumpMessage = {
            info: { role: "assistant", time: { created: 1, completed: 2 } },
            parts: [],
        };
        const page = concatSessionMessages([text("user", "hello"), completedEmpty], 1000);
        expect(page.endIndex).toBe(1);
        expect(page.hasMore).toBe(false);
    });

    test("an empty unfinished assistant message that is not trailing is consumed", () => {
        const aborted: DumpMessage = {
            info: { role: "assistant", time: { created: 1 } },
            parts: [],
        };
        const page = concatSessionMessages([aborted, text("user", "later")], 1000);
        expect(page.output).toBe("[1] User: later");
        expect(page.endIndex).toBe(1);
        expect(page.hasMore).toBe(false);
    });

    test("a first message that exceeds the budget throws instead of reporting it consumed", () => {
        const big = text("user", "word ".repeat(200));
        expect(() => concatSessionMessages([big], 10)).toThrow(
            /Message 0 needs \d+ tokens on its own, which exceeds the budget of 10/,
        );
        // Same message reached by resuming after an admitted first page.
        const messages = [text("user", "short"), big];
        const first = concatSessionMessages(messages, 10);
        expect(first.endIndex).toBe(0);
        expect(first.hasMore).toBe(true);
        expect(() => concatSessionMessages(messages, 10, first.endIndex + 1)).toThrow(
            /Message 1 needs/,
        );
    });

    test("a first tool run whose summary exceeds the budget throws", () => {
        expect(() => concatSessionMessages([toolsOnly(3)], 1)).toThrow(/Message 0 needs/);
        expect(() => concatSessionMessages([toolsOnly(3), text("user", "x")], 1)).toThrow(
            /Message 0 needs/,
        );
    });

    test("an offset past the end reports no progress and no more pages", () => {
        const messages = [text("user", "hello")];
        const page = concatSessionMessages(messages, 1000, 1);
        expect(page.output).toBe("");
        expect(page.startIndex).toBe(1);
        expect(page.endIndex).toBe(0);
        expect(page.messagesWithContent).toBe(0);
        expect(page.hasMore).toBe(false);
    });
});

describe("message anchors", () => {
    function anchored(timeCreated: number, id: string, body: string): DumpMessage {
        return {
            info: { role: "user", id, timeCreated },
            parts: [{ type: "text", text: body }],
        };
    }

    test("indexAfterAnchor orders by (timeCreated, id) and skips a deleted anchor", () => {
        const messages = [
            anchored(10, "a", "one"),
            anchored(20, "b", "two"),
            anchored(20, "c", "three"),
            anchored(30, "d", "four"),
        ];
        expect(indexAfterAnchor(messages, { timeCreated: 20, id: "b" })).toBe(2);
        expect(indexAfterAnchor(messages, { timeCreated: 20, id: "c" })).toBe(3);
        expect(indexAfterAnchor(messages, { timeCreated: 30, id: "d" })).toBe(4);
        // The anchor itself was deleted; resumption lands on the next surviving message.
        expect(indexAfterAnchor(messages, { timeCreated: 15, id: "gone" })).toBe(1);
        expect(indexAfterAnchor(messages, { timeCreated: 0, id: "" })).toBe(0);
    });

    test("formatAnchor and parseAnchor round-trip and reject malformed input", () => {
        const anchor = { timeCreated: 1788847243651, id: "msg_abc:with:colons" };
        expect(parseAnchor(formatAnchor(anchor))).toEqual(anchor);
        expect(() => parseAnchor("nope")).toThrow(/--after expects/);
        expect(() => parseAnchor("12:")).toThrow(/--after expects/);
    });
});

describe("runContextConcat", () => {
    const savedDbPath = process.env.OPENCODE_DB_PATH;
    let dir: string | undefined;

    afterEach(() => {
        if (savedDbPath === undefined) delete process.env.OPENCODE_DB_PATH;
        else process.env.OPENCODE_DB_PATH = savedDbPath;
        if (dir) rmSync(dir, { recursive: true, force: true });
        dir = undefined;
    });

    function seedDatabase(): { path: string; db: Database } {
        dir = mkdtempSync(join(tmpdir(), "eidnara-concat-"));
        const path = join(dir, "opencode.db");
        const db = new Database(path);
        db.exec(`
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
        `);
        const insertMessage = db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, 'ses', ?, ?, ?)",
        );
        const insertPart = db.prepare(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, 'ses', ?, ?, ?)",
        );
        for (const [i, body] of ["one", "two", "three", "four"].entries()) {
            const id = `m${i}`;
            const t = 100 + i;
            insertMessage.run(
                id,
                t,
                t,
                JSON.stringify({ role: "user", time: { created: t, completed: t } }),
            );
            insertPart.run(`p${i}`, id, t, t, JSON.stringify({ type: "text", text: body }));
        }
        process.env.OPENCODE_DB_PATH = path;
        return { path, db };
    }

    test("endAnchor resumes after deletions of earlier messages", () => {
        const { db } = seedDatabase();
        const first = runContextConcat("ses", 13);
        expect(first.output).toBe("[0] User: one\n[1] User: two");
        expect(first.hasMore).toBe(true);
        expect(first.endAnchor).toEqual({ timeCreated: 101, id: "m1" });

        // A compaction cleanup removes the first message before the next page is read.
        db.prepare("DELETE FROM part WHERE message_id = 'm0'").run();
        db.prepare("DELETE FROM message WHERE id = 'm0'").run();

        const second = runContextConcat("ses", 1000, first.endAnchor);
        expect(second.output).toBe("[1] User: three\n[2] User: four");
        expect(second.hasMore).toBe(false);
        expect(second.endAnchor).toEqual({ timeCreated: 103, id: "m3" });
    });

    test("endAnchor is null when the page consumed nothing", () => {
        seedDatabase();
        const page = runContextConcat("ses", 1000, { timeCreated: 103, id: "m3" });
        expect(page.output).toBe("");
        expect(page.hasMore).toBe(false);
        expect(page.endAnchor).toBeNull();
    });
});
