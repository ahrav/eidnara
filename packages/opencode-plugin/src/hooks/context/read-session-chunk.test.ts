/// <reference types="bun-types" />

import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    estimateTokens,
    installTokenizerForTest,
    resetTokenEstimatorForTest,
} from "../../shared/token-estimator";
import {
    blockTokenMemoStatsForTest,
    getProtectedTailStartOrdinal,
    getRawSessionMessageCount,
    getRawSessionMessageIdsThrough,
    primeTailRawMessageCache,
    readRawSessionMessageById,
    readRawSessionMessageOrdinalPage,
    readRawSessionMessages,
    readSessionChunk,
    setRawMessageProvider,
    withRawMessageProvider,
    withRawSessionMessageCache,
} from "./read-session-chunk";
import type { RawMessage } from "./read-session-raw";

const tempDirs: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

afterEach(() => {
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

function useTempDataHome(prefix: string): void {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
}

function createOpenCodeDb(sessionId: string): void {
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
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        message_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        time_created INTEGER NOT NULL,
        time_updated INTEGER NOT NULL,
        data TEXT NOT NULL
      );
    `);

        const insertMessage = db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        );
        const insertPart = db.prepare(
            "INSERT INTO part (message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        );

        const messages = [
            { id: "m-1", role: "user", part: { type: "text", text: "hello" } },
            { id: "m-2", role: "assistant", part: { type: "tool", callID: "call-1" } },
            { id: "m-3", role: "assistant", part: { type: "text", text: "done" } },
        ];

        messages.forEach((message, index) => {
            const timestamp = index + 1;
            insertMessage.run(
                message.id,
                sessionId,
                timestamp,
                timestamp,
                JSON.stringify({ id: message.id, role: message.role, sessionID: sessionId }),
            );
            insertPart.run(
                message.id,
                sessionId,
                timestamp,
                timestamp,
                JSON.stringify(message.part),
            );
        });
    } finally {
        closeQuietly(db);
    }
}

function createOpenCodeDbWithMessages(
    sessionId: string,
    messages: Array<{ id: string; role: string; part: Record<string, unknown> }>,
): void {
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
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        message_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        time_created INTEGER NOT NULL,
        time_updated INTEGER NOT NULL,
        data TEXT NOT NULL
      );
    `);

        const insertMessage = db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        );
        const insertPart = db.prepare(
            "INSERT INTO part (message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        );

        messages.forEach((message, index) => {
            const timestamp = index + 1;
            insertMessage.run(
                message.id,
                sessionId,
                timestamp,
                timestamp,
                JSON.stringify({ id: message.id, role: message.role, sessionID: sessionId }),
            );
            insertPart.run(
                message.id,
                sessionId,
                timestamp,
                timestamp,
                JSON.stringify(message.part),
            );
        });
    } finally {
        closeQuietly(db);
    }
}

function appendOpenCodeMessage(
    sessionId: string,
    message: { id: string; role: string; part: Record<string, unknown> },
    timestamp: number,
): void {
    const dbPath = join(process.env.XDG_DATA_HOME!, "opencode", "opencode.db");
    const db = new Database(dbPath);
    try {
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        ).run(
            message.id,
            sessionId,
            timestamp,
            timestamp,
            JSON.stringify({ id: message.id, role: message.role, sessionID: sessionId }),
        );
        db.prepare(
            "INSERT INTO part (message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        ).run(message.id, sessionId, timestamp, timestamp, JSON.stringify(message.part));
    } finally {
        closeQuietly(db);
    }
}

/** A row whose `data` is not JSON consumes an ordinal slot but the raw reader emits no message for it. */
function appendMalformedOpenCodeMessage(sessionId: string, id: string, timestamp: number): void {
    const dbPath = join(process.env.XDG_DATA_HOME!, "opencode", "opencode.db");
    const db = new Database(dbPath);
    try {
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        ).run(id, sessionId, timestamp, timestamp, "not json");
    } finally {
        closeQuietly(db);
    }
}

function providerMessage(id: string, ordinal: number, createdAt: number): RawMessage {
    return {
        id,
        ordinal,
        role: "user",
        parts: [{ type: "text", text: `message ${id}` }],
        createdAt,
        version: null,
    };
}

describe("readSessionChunk", () => {
    it("reads raw OpenCode messages with stable ordinals and ids", () => {
        useTempDataHome("read-session-chunk-");
        createOpenCodeDb("ses-raw");

        const chunk = readSessionChunk("ses-raw", 10_000, 1);

        expect(chunk.startIndex).toBe(1);
        expect(chunk.endIndex).toBe(3);
        expect(chunk.startMessageId).toBe("m-1");
        expect(chunk.endMessageId).toBe("m-3");
        expect(chunk.text).toContain("[1] U: hello");
        expect(chunk.text).toContain("[2-3] A: done");
        expect(chunk.text).not.toContain("msg_");
        expect(chunk.text).not.toContain("tool call");
    });

    it("keeps a media-only user turn as a placeholder instead of noise", () => {
        useTempDataHome("read-session-media-turn-");
        createOpenCodeDbWithMessages("ses-media", [
            {
                id: "m-1",
                role: "user",
                part: { type: "file", mime: "image/png", filename: "screen.png", url: "data:..." },
            },
            { id: "m-2", role: "assistant", part: { type: "text", text: "I see the screenshot" } },
        ]);

        const chunk = readSessionChunk("ses-media", 10_000, 1);

        expect(chunk.text).toContain("[1] U: [media:image image/png screen.png]");
        expect(chunk.text).toContain("[2] A: I see the screenshot");
        expect(chunk.toolOnlyRanges).toEqual([]);
    });

    it("drops ignored text from a turn that also carries real text", () => {
        useTempDataHome("read-session-mixed-ignored-");
        const mixed: RawMessage[] = [
            {
                id: "m-1",
                ordinal: 1,
                role: "user",
                parts: [
                    { type: "text", text: "## Eidnara Status", ignored: true },
                    { type: "text", text: "please fix the auth bug" },
                ],
                createdAt: 1,
                version: null,
            },
        ];
        withRawMessageProvider("ses-mixed", { readMessages: () => mixed }, () => {
            const chunk = readSessionChunk("ses-mixed", 10_000, 1);
            expect(chunk.text).toContain("please fix the auth bug");
            expect(chunk.text).not.toContain("Eidnara Status");
        });
    });

    it("treats system rows as noise rather than emitting them as blocks", () => {
        useTempDataHome("read-session-system-row-");
        createOpenCodeDbWithMessages("ses-system", [
            { id: "m-1", role: "system", part: { type: "text", text: "You are a helpful bot." } },
            { id: "m-2", role: "user", part: { type: "text", text: "hello" } },
            { id: "m-3", role: "assistant", part: { type: "text", text: "hi there" } },
        ]);

        const chunk = readSessionChunk("ses-system", 10_000, 1);

        expect(chunk.text).not.toContain("helpful bot");
        expect(chunk.text).not.toMatch(/S:/);
        // The system row's ordinal is absorbed into the next block's range.
        expect(chunk.text).toContain("[1-2] U: hello");
    });

    it("summarizes a flat OpenCode tool part through the same field fallbacks as the codec", () => {
        useTempDataHome("read-session-flat-tool-");
        createOpenCodeDbWithMessages("ses-flat-tool", [
            { id: "m-1", role: "user", part: { type: "text", text: "read it" } },
            {
                id: "m-2",
                role: "assistant",
                part: {
                    type: "tool",
                    tool: "read",
                    status: "completed",
                    input: { filePath: "/src/main.rs" },
                    output: "fn main() {}",
                },
            },
        ]);

        const chunk = readSessionChunk("ses-flat-tool", 10_000, 1);

        expect(chunk.text).toContain("TC: read(/src/main.rs)");
        expect(chunk.toolOnlyRanges).toEqual([{ start: 2, end: 2 }]);
    });

    it("charges the separator between blocks against the budget", () => {
        useTempDataHome("read-session-separator-budget-");
        createOpenCodeDbWithMessages("ses-separator", [
            { id: "m-1", role: "user", part: { type: "text", text: "a" } },
            { id: "m-2", role: "assistant", part: { type: "text", text: "b" } },
            { id: "m-3", role: "user", part: { type: "text", text: "c" } },
        ]);

        const unbounded = readSessionChunk("ses-separator", 100_000, 1);
        const joined = unbounded.text;
        const blocks = joined.split("\n");
        expect(blocks).toHaveLength(3);
        const blocksOnly = blocks.reduce((sum, line) => sum + estimateTokens(line), 0);
        expect(unbounded.tokenEstimate).toBe(blocksOnly + 2 * estimateTokens("\n"));
    });

    it("reuses cached raw messages within nested cache scopes and clears afterward", () => {
        useTempDataHome("read-session-cache-scope-");
        createOpenCodeDbWithMessages("ses-cache", [
            { id: "m-1", role: "user", part: { type: "text", text: "turn 1" } },
        ]);

        withRawSessionMessageCache(() => {
            const outerRead = readRawSessionMessages("ses-cache");
            expect(outerRead).toHaveLength(1);

            withRawSessionMessageCache(() => {
                const nestedRead = readRawSessionMessages("ses-cache");
                expect(nestedRead).toBe(outerRead);

                appendOpenCodeMessage(
                    "ses-cache",
                    { id: "m-2", role: "assistant", part: { type: "text", text: "turn 2" } },
                    2,
                );

                const nestedCachedRead = readRawSessionMessages("ses-cache");
                expect(nestedCachedRead).toBe(outerRead);
                expect(nestedCachedRead).toHaveLength(1);
            });

            const outerCachedRead = readRawSessionMessages("ses-cache");
            expect(outerCachedRead).toBe(outerRead);
            expect(outerCachedRead).toHaveLength(1);
        });

        const freshRead = readRawSessionMessages("ses-cache");
        expect(freshRead).toHaveLength(2);
    });

    it("keeps the raw-message cache alive until an async scope settles", async () => {
        useTempDataHome("read-session-async-cache-scope-");
        createOpenCodeDbWithMessages("ses-async-cache", [
            { id: "m-1", role: "user", part: { type: "text", text: "turn 1" } },
        ]);

        const reads = await withRawSessionMessageCache(async () => {
            const beforeAwait = readRawSessionMessages("ses-async-cache");
            await Promise.resolve();
            appendOpenCodeMessage(
                "ses-async-cache",
                { id: "m-2", role: "assistant", part: { type: "text", text: "turn 2" } },
                2,
            );
            const afterAwait = readRawSessionMessages("ses-async-cache");
            return { beforeAwait, afterAwait };
        });

        expect(reads.afterAwait).toBe(reads.beforeAwait);
        expect(reads.afterAwait).toHaveLength(1);
        expect(readRawSessionMessages("ses-async-cache")).toHaveLength(2);
    });

    it("clears the raw-message cache when an async scope rejects", async () => {
        useTempDataHome("read-session-async-cache-reject-");
        createOpenCodeDbWithMessages("ses-async-reject", [
            { id: "m-1", role: "user", part: { type: "text", text: "turn 1" } },
        ]);

        await expect(
            withRawSessionMessageCache(async () => {
                readRawSessionMessages("ses-async-reject");
                await Promise.resolve();
                throw new Error("boom");
            }),
        ).rejects.toThrow("boom");

        appendOpenCodeMessage(
            "ses-async-reject",
            { id: "m-2", role: "assistant", part: { type: "text", text: "turn 2" } },
            2,
        );
        expect(readRawSessionMessages("ses-async-reject")).toHaveLength(2);
    });

    it("keeps the raw-message cache alive until every overlapping async scope settles", async () => {
        useTempDataHome("read-session-overlap-cache-scope-");
        createOpenCodeDbWithMessages("ses-overlap-cache", [
            { id: "m-1", role: "user", part: { type: "text", text: "turn 1" } },
        ]);

        let releaseFirst!: () => void;
        const firstSettled = new Promise<void>((resolve) => {
            releaseFirst = resolve;
        });
        let releaseSecond!: () => void;
        const secondSettled = new Promise<void>((resolve) => {
            releaseSecond = resolve;
        });

        const first = withRawSessionMessageCache(async () => {
            readRawSessionMessages("ses-overlap-cache");
            await firstSettled;
        });
        const second = withRawSessionMessageCache(async () => {
            const beforeFirstSettles = readRawSessionMessages("ses-overlap-cache");
            await secondSettled;
            const afterFirstSettles = readRawSessionMessages("ses-overlap-cache");
            return { beforeFirstSettles, afterFirstSettles };
        });

        releaseFirst();
        await first;
        appendOpenCodeMessage(
            "ses-overlap-cache",
            { id: "m-2", role: "assistant", part: { type: "text", text: "turn 2" } },
            2,
        );
        releaseSecond();
        const reads = await second;

        expect(reads.afterFirstSettles).toBe(reads.beforeFirstSettles);
        expect(reads.afterFirstSettles).toHaveLength(1);
        expect(readRawSessionMessages("ses-overlap-cache")).toHaveLength(2);
    });

    it("drops a session's cached rows when a provider is registered or released", () => {
        useTempDataHome("read-session-provider-cache-");
        createOpenCodeDbWithMessages("ses-provider-cache", [
            { id: "db-1", role: "user", part: { type: "text", text: "from the database" } },
        ]);
        const provider = { readMessages: () => [providerMessage("provider-1", 1, 1)] };

        withRawSessionMessageCache(() => {
            expect(readRawSessionMessages("ses-provider-cache").map((m) => m.id)).toEqual(["db-1"]);

            withRawMessageProvider("ses-provider-cache", provider, () => {
                expect(readRawSessionMessages("ses-provider-cache").map((m) => m.id)).toEqual([
                    "provider-1",
                ]);
            });

            expect(readRawSessionMessages("ses-provider-cache").map((m) => m.id)).toEqual(["db-1"]);
        });
    });

    it("restores the outer provider when a nested provider scope ends", () => {
        useTempDataHome("read-session-nested-provider-");
        const outer = { readMessages: () => [providerMessage("outer-1", 1, 1)] };
        const inner = { readMessages: () => [providerMessage("inner-1", 1, 1)] };

        withRawMessageProvider("ses-nested", outer, () => {
            expect(readRawSessionMessages("ses-nested").map((m) => m.id)).toEqual(["outer-1"]);
            withRawMessageProvider("ses-nested", inner, () => {
                expect(readRawSessionMessages("ses-nested").map((m) => m.id)).toEqual(["inner-1"]);
            });
            expect(readRawSessionMessages("ses-nested").map((m) => m.id)).toEqual(["outer-1"]);
        });

        expect(readRawSessionMessages("ses-nested")).toEqual([]);
    });

    it("classifies tool arcs by the registered provider's shape", () => {
        useTempDataHome("read-session-provider-shape-");
        const piMessages: RawMessage[] = [
            {
                id: "pi-call",
                ordinal: 1,
                role: "assistant",
                parts: [{ type: "toolCall", id: "tc1", name: "read", arguments: {} }],
                createdAt: 1,
                version: null,
            },
            {
                id: "pi-result",
                ordinal: 2,
                role: "user",
                parts: [
                    {
                        role: "toolResult",
                        toolCallId: "tc1",
                        toolName: "read",
                        content: [{ type: "text", text: "ok" }],
                    },
                ],
                createdAt: 2,
                version: null,
            },
        ];

        const piShaped = {
            readMessages: () => piMessages,
            providerShapeVersion: "pi-folded-v1" as const,
        };
        withRawMessageProvider("ses-pi-shape", piShaped, () => {
            expect(readSessionChunk("ses-pi-shape", 100_000, 1).completedToolArcs).toEqual([
                { start: 1, end: 2 },
            ]);
        });

        // Without a declared shape the OpenCode rules apply, and they do not recognize Pi parts.
        const defaultShaped = { readMessages: () => piMessages };
        withRawMessageProvider("ses-default-shape", defaultShaped, () => {
            expect(readSessionChunk("ses-default-shape", 100_000, 1).completedToolArcs).toEqual([]);
        });
    });

    it("releases each provider on its own cleanup when scopes overlap out of order", () => {
        useTempDataHome("read-session-overlap-provider-");
        const first = { readMessages: () => [providerMessage("first-1", 1, 1)] };
        const second = { readMessages: () => [providerMessage("second-1", 1, 1)] };

        const releaseFirst = setRawMessageProvider("ses-overlap", first);
        const releaseSecond = setRawMessageProvider("ses-overlap", second);

        releaseFirst();
        expect(readRawSessionMessages("ses-overlap").map((m) => m.id)).toEqual(["second-1"]);

        releaseSecond();
        expect(readRawSessionMessages("ses-overlap")).toEqual([]);

        releaseSecond();
        expect(readRawSessionMessages("ses-overlap")).toEqual([]);
    });

    it("keeps a stale release from unregistering a re-registered provider", () => {
        useTempDataHome("read-session-stale-release-");
        const provider = { readMessages: () => [providerMessage("p-1", 1, 1)] };

        const staleRelease = setRawMessageProvider("ses-stale-release", provider);
        staleRelease();
        const liveRelease = setRawMessageProvider("ses-stale-release", provider);

        staleRelease();
        expect(readRawSessionMessages("ses-stale-release").map((m) => m.id)).toEqual(["p-1"]);

        liveRelease();
        expect(readRawSessionMessages("ses-stale-release")).toEqual([]);
    });

    it("keeps the active provider's cached rows when an inactive registration is released", () => {
        useTempDataHome("read-session-inactive-release-cache-");
        const outer = { readMessages: () => [providerMessage("outer-1", 1, 1)] };
        const inner = { readMessages: () => [providerMessage("inner-1", 1, 1)] };

        withRawSessionMessageCache(() => {
            const releaseOuter = setRawMessageProvider("ses-inactive-release", outer);
            const releaseInner = setRawMessageProvider("ses-inactive-release", inner);

            const cached = readRawSessionMessages("ses-inactive-release");
            expect(cached.map((m) => m.id)).toEqual(["inner-1"]);

            releaseOuter();
            expect(readRawSessionMessages("ses-inactive-release")).toBe(cached);

            releaseInner();
            expect(readRawSessionMessages("ses-inactive-release")).toEqual([]);
        });
    });

    it("pages provider ordinal entries with the ordering the anchor filter uses", () => {
        // "B" sorts before "a" by code unit (66 < 97) but after it under locale collation.
        const provider = {
            readMessages: () => [providerMessage("a", 1, 5), providerMessage("B", 2, 5)],
        };

        withRawMessageProvider("ses-ordinal-page", provider, () => {
            const firstPage = readRawSessionMessageOrdinalPage("ses-ordinal-page", null, 1);
            expect(firstPage.map((entry) => entry.id)).toEqual(["B"]);

            const secondPage = readRawSessionMessageOrdinalPage(
                "ses-ordinal-page",
                { timeCreated: firstPage[0].timeCreated, id: firstPage[0].id },
                1,
            );
            expect(secondPage.map((entry) => entry.id)).toEqual(["a"]);

            const thirdPage = readRawSessionMessageOrdinalPage(
                "ses-ordinal-page",
                { timeCreated: secondPage[0].timeCreated, id: secondPage[0].id },
                1,
            );
            expect(thirdPage).toEqual([]);
        });
    });

    it("returns raw message ids through an ordinal", () => {
        useTempDataHome("read-session-ids-");
        createOpenCodeDb("ses-raw");

        expect(getRawSessionMessageIdsThrough("ses-raw", 2)).toEqual(["m-1", "m-2"]);
    });

    it("bounds the block-token memo by retained characters as well as entries", () => {
        useTempDataHome("read-session-memo-chars-");
        // Six alternating-role blocks of 800K characters each: 4.8M total against a 4M budget.
        // Varied filler keeps byte-pair tokenization linear; a single repeated character is not.
        const filler = "lorem ipsum dolor sit amet ".repeat(30_000);
        const messages = Array.from({ length: 6 }, (_, index) => ({
            id: `m-${index + 1}`,
            role: index % 2 === 0 ? "user" : "assistant",
            part: { type: "text", text: `block ${index} ${filler}` },
        }));
        createOpenCodeDbWithMessages("ses-memo-chars", messages);

        const chunk = readSessionChunk("ses-memo-chars", 100_000_000, 1);
        expect(chunk.messageCount).toBe(6);

        const stats = blockTokenMemoStatsForTest();
        expect(stats.chars).toBeLessThanOrEqual(stats.maxChars);
        expect(stats.entries).toBeLessThan(6);
    });

    it("re-tokenizes memoized blocks after the tokenizer becomes available", () => {
        useTempDataHome("read-session-memo-generation-");
        createOpenCodeDbWithMessages("ses-memo-generation", [
            { id: "m-1", role: "user", part: { type: "text", text: "count me twice" } },
        ]);

        try {
            installTokenizerForTest(null);
            const heuristic = readSessionChunk("ses-memo-generation", 100_000, 1).tokenEstimate;

            installTokenizerForTest({ encode: () => [1, 2, 3] });
            const tokenized = readSessionChunk("ses-memo-generation", 100_000, 1).tokenEstimate;

            expect(heuristic).toBe(Math.ceil("[1] U: count me twice".length / 3.5));
            expect(tokenized).toBe(3);
        } finally {
            resetTokenEstimatorForTest();
        }
    });

    it("extracts commit hashes into compact assistant block metadata", () => {
        useTempDataHome("read-session-commits-");
        createOpenCodeDbWithMessages("ses-commits", [
            { id: "m-1", role: "user", part: { type: "text", text: "ship it" } },
            {
                id: "m-2",
                role: "assistant",
                part: {
                    type: "text",
                    text: "Done. `4301a084` on feat, cherry-picked `7e80a1a7` to integrate.",
                },
            },
            {
                id: "m-3",
                role: "assistant",
                part: { type: "text", text: "Build passes after commit `4301a084`." },
            },
        ]);

        const chunk = readSessionChunk("ses-commits", 10_000, 1);

        expect(chunk.text).toContain("[2-3] A: commits: 4301a084, 7e80a1a7");
        expect(chunk.text).not.toContain("`4301a084`");
        expect(chunk.text).not.toContain("`7e80a1a7`");
    });

    describe("getProtectedTailStartOrdinal", () => {
        it("returns 1 when exactly 5 user turns exist", () => {
            //#given
            useTempDataHome("protected-tail-ordinal-");
            createOpenCodeDbWithMessages("ses-tail", [
                { id: "m-1", role: "user", part: { type: "text", text: "turn 1" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "reply 1" } },
                { id: "m-3", role: "user", part: { type: "text", text: "turn 2" } },
                { id: "m-4", role: "assistant", part: { type: "text", text: "reply 2" } },
                { id: "m-5", role: "user", part: { type: "text", text: "turn 3" } },
                { id: "m-6", role: "assistant", part: { type: "text", text: "reply 3" } },
            ]);

            //#when
            const ordinal = getProtectedTailStartOrdinal("ses-tail");

            // All 5 user turns are protected.
            expect(ordinal).toBe(1);
        });

        it("returns ordinal of the 5th-to-last user message when 6+ user turns exist", () => {
            //#given
            useTempDataHome("protected-tail-6turns-");
            createOpenCodeDbWithMessages("ses-6turns", [
                { id: "m-1", role: "user", part: { type: "text", text: "turn 1" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "reply 1" } },
                { id: "m-3", role: "user", part: { type: "text", text: "turn 2" } },
                { id: "m-4", role: "assistant", part: { type: "text", text: "reply 2" } },
                { id: "m-5", role: "user", part: { type: "text", text: "turn 3" } },
                { id: "m-6", role: "assistant", part: { type: "text", text: "reply 3" } },
                { id: "m-7", role: "user", part: { type: "text", text: "turn 4" } },
                { id: "m-8", role: "assistant", part: { type: "text", text: "reply 4" } },
                { id: "m-9", role: "user", part: { type: "text", text: "turn 5" } },
                { id: "m-10", role: "assistant", part: { type: "text", text: "reply 5" } },
                { id: "m-11", role: "user", part: { type: "text", text: "turn 6" } },
                { id: "m-12", role: "assistant", part: { type: "text", text: "reply 6" } },
            ]);

            //#when
            const ordinal = getProtectedTailStartOrdinal("ses-6turns");

            // The 5th-to-last user message is m-3 at ordinal 3.
            expect(ordinal).toBe(3);
        });

        it("returns 1 when fewer than 5 user turns exist", () => {
            //#given
            useTempDataHome("protected-tail-few-");
            createOpenCodeDbWithMessages("ses-few", [
                { id: "m-1", role: "user", part: { type: "text", text: "only turn" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "reply" } },
                { id: "m-3", role: "user", part: { type: "text", text: "second turn" } },
            ]);

            //#when
            const ordinal = getProtectedTailStartOrdinal("ses-few");

            // All messages are protected.
            expect(ordinal).toBe(1);
        });

        it("ignores system-reminder and ignored synthetic user messages when counting protected tail", () => {
            //#given
            useTempDataHome("protected-tail-ignore-synthetic-");
            createOpenCodeDbWithMessages("ses-synthetic", [
                { id: "m-1", role: "user", part: { type: "text", text: "real turn 1" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "reply 1" } },
                {
                    id: "m-3",
                    role: "user",
                    part: {
                        type: "text",
                        text: "<system-reminder>background finished</system-reminder>\n<!-- OMO_INTERNAL_INITIATOR -->",
                    },
                },
                { id: "m-4", role: "assistant", part: { type: "text", text: "reply 2" } },
                {
                    id: "m-5",
                    role: "user",
                    part: { type: "text", text: "## Eidnara Status", ignored: true },
                },
                { id: "m-6", role: "assistant", part: { type: "text", text: "reply 3" } },
                { id: "m-7", role: "user", part: { type: "text", text: "real turn 2" } },
                { id: "m-8", role: "assistant", part: { type: "text", text: "reply 4" } },
                { id: "m-9", role: "user", part: { type: "text", text: "real turn 3" } },
                { id: "m-10", role: "assistant", part: { type: "text", text: "reply 5" } },
                { id: "m-11", role: "user", part: { type: "text", text: "real turn 4" } },
            ]);

            //#when
            const ordinal = getProtectedTailStartOrdinal("ses-synthetic");

            // All 4 real user turns are protected because there are fewer than 5.
            expect(ordinal).toBe(1);
        });

        it("still counts mixed user messages when real user text remains after stripping reminders", () => {
            //#given
            useTempDataHome("protected-tail-mixed-user-");
            createOpenCodeDbWithMessages("ses-mixed", [
                { id: "m-1", role: "user", part: { type: "text", text: "real turn 1" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "reply 1" } },
                { id: "m-3", role: "user", part: { type: "text", text: "real turn 2" } },
                { id: "m-4", role: "assistant", part: { type: "text", text: "reply 2" } },
                {
                    id: "m-5",
                    role: "user",
                    part: {
                        type: "text",
                        text: "<system-reminder>background finished</system-reminder>\nPlease also keep this architectural concern in mind.",
                    },
                },
                { id: "m-6", role: "assistant", part: { type: "text", text: "reply 3" } },
                { id: "m-7", role: "user", part: { type: "text", text: "real turn 4" } },
            ]);

            //#when
            const ordinal = getProtectedTailStartOrdinal("ses-mixed");

            // All 4 meaningful user turns are protected because there are fewer than 5.
            expect(ordinal).toBe(1);
        });
    });

    describe("readSessionChunk with eligibleEndOrdinal", () => {
        it("stops before the protected tail messages", () => {
            //#given
            useTempDataHome("read-session-eligible-end-");
            createOpenCodeDbWithMessages("ses-eligible", [
                { id: "m-1", role: "user", part: { type: "text", text: "old work" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "done" } },
                { id: "m-3", role: "user", part: { type: "text", text: "protected turn 1" } },
                { id: "m-4", role: "assistant", part: { type: "text", text: "protected reply 1" } },
                { id: "m-5", role: "user", part: { type: "text", text: "protected turn 2" } },
            ]);

            // The eligible end is ordinal 3 because the protected tail starts at m-3.
            const chunk = readSessionChunk("ses-eligible", 100_000, 1, 3);

            // The eligible range includes only m-1 and m-2.
            expect(chunk.text).toContain("old work");
            expect(chunk.text).toContain("done");
            expect(chunk.text).not.toContain("protected turn");
            expect(chunk.endIndex).toBe(2);
            expect(chunk.hasMore).toBe(false);
        });

        it("reports hasMore false when all eligible messages fit within the budget", () => {
            //#given
            useTempDataHome("read-session-hasmore-");
            createOpenCodeDbWithMessages("ses-hasmore", [
                { id: "m-1", role: "user", part: { type: "text", text: "eligible" } },
                { id: "m-2", role: "user", part: { type: "text", text: "tail turn 1" } },
                { id: "m-3", role: "user", part: { type: "text", text: "tail turn 2" } },
                { id: "m-4", role: "user", part: { type: "text", text: "tail turn 3" } },
            ]);

            // The eligible end excludes m-2 and later messages.
            const chunk = readSessionChunk("ses-hasmore", 100_000, 1, 2);

            //#then
            expect(chunk.messageCount).toBe(1);
            expect(chunk.hasMore).toBe(false);
        });

        it("reports hasMore false when the remaining eligible tail is only filtered noise", () => {
            //#given
            useTempDataHome("read-session-noise-tail-");
            createOpenCodeDbWithMessages("ses-noise-tail", [
                { id: "m-1", role: "user", part: { type: "text", text: "eligible work" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "done" } },
                {
                    id: "m-3",
                    role: "user",
                    part: { type: "text", text: "## Eidnara Status", ignored: true },
                },
                {
                    id: "m-4",
                    role: "user",
                    part: {
                        type: "text",
                        text: "<system-reminder>background finished</system-reminder>",
                    },
                },
            ]);

            //#when
            const chunk = readSessionChunk("ses-noise-tail", 100_000, 1);

            expect(chunk.endIndex).toBe(2);
            expect(chunk.text).toContain("eligible work");
            expect(chunk.text).not.toContain("Magic Status");
            expect(chunk.hasMore).toBe(false);
        });

        it("keeps hasMore true when budget-blocked content precedes trailing noise", () => {
            //#given
            useTempDataHome("read-session-blocked-before-noise-");
            createOpenCodeDbWithMessages("ses-blocked-before-noise", [
                { id: "m-1", role: "user", part: { type: "text", text: "first content" } },
                {
                    id: "m-2",
                    role: "assistant",
                    part: { type: "text", text: "second content beyond budget" },
                },
                {
                    id: "m-3",
                    role: "user",
                    part: { type: "text", text: "## Eidnara Status", ignored: true },
                },
            ]);

            //#when
            const chunk = readSessionChunk("ses-blocked-before-noise", 1, 1);

            //#then
            expect(chunk.endIndex).toBe(1);
            expect(chunk.text).toContain("first content");
            expect(chunk.text).not.toContain("second content");
            expect(chunk.hasMore).toBe(true);
        });

        it("reports hasMore false when the primed tail ends in an omitted malformed row", () => {
            //#given
            useTempDataHome("read-session-omitted-tail-slot-");
            createOpenCodeDbWithMessages("ses-omitted-slot", [
                { id: "m-1", role: "user", part: { type: "text", text: "boundary" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "after boundary" } },
            ]);
            appendMalformedOpenCodeMessage("ses-omitted-slot", "m-3", 3);

            withRawSessionMessageCache(() => {
                expect(
                    primeTailRawMessageCache({
                        sessionId: "ses-omitted-slot",
                        lastCompartmentEnd: 1,
                        anchorMessageId: "m-1",
                    }),
                ).toBe(true);

                //#when
                const chunk = readSessionChunk("ses-omitted-slot", 100_000, 2);

                //#then
                expect(chunk.endIndex).toBe(2);
                expect(chunk.text).toContain("after boundary");
                expect(chunk.hasMore).toBe(false);

                const next = readSessionChunk("ses-omitted-slot", 100_000, chunk.endIndex + 1);
                expect(next.messageCount).toBe(0);
                expect(next.hasMore).toBe(false);
            });
        });

        it("counts raw messages from a database that holds a malformed row", () => {
            //#given
            useTempDataHome("read-session-malformed-count-");
            createOpenCodeDbWithMessages("ses-malformed-count", [
                { id: "m-1", role: "user", part: { type: "text", text: "hello" } },
                { id: "m-2", role: "assistant", part: { type: "text", text: "hi" } },
            ]);
            appendMalformedOpenCodeMessage("ses-malformed-count", "m-3", 3);

            //#then
            expect(getRawSessionMessageCount("ses-malformed-count")).toBe(3);
        });

        it("reads a message by id past an earlier malformed row", () => {
            //#given
            useTempDataHome("read-session-malformed-by-id-");
            createOpenCodeDbWithMessages("ses-malformed-by-id", [
                { id: "m-1", role: "user", part: { type: "text", text: "hello" } },
            ]);
            appendMalformedOpenCodeMessage("ses-malformed-by-id", "m-2", 2);
            appendOpenCodeMessage(
                "ses-malformed-by-id",
                { id: "m-3", role: "assistant", part: { type: "text", text: "hi" } },
                3,
            );

            //#when
            const message = readRawSessionMessageById("ses-malformed-by-id", "m-3");

            //#then
            expect(message?.id).toBe("m-3");
            expect(message?.ordinal).toBe(3);
        });
    });
});
