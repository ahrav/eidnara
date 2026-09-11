import assert from "node:assert/strict";
import { mkdtempSync, renameSync, rmSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Database, detectSqliteRuntime, type Statement } from "../../../shared/sqlite";
import {
    closeReadOnlySessionDb,
    getMessageTimesFromOpenCodeDb,
    isMidTurn,
    withReadOnlySessionDb,
} from "../read-session-db";
import { readRawSessionMessagePageFromDb } from "../read-session-raw";

const dir = mkdtempSync(join(tmpdir(), "session-db-cache-"));
const path = join(dir, "session.db");
const replacement = join(dir, "replacement.db");
const originalOverride = process.env.OPENCODE_DB;
process.env.OPENCODE_DB = path;
const chunkSession = `ses_${"0".repeat(26)}`;
const growingChunks = process.argv[2] === "growing-chunks";
const chunkIds = Array.from(
    { length: growingChunks ? 870 : 800 },
    (_, i) => `msg_${i.toString(32).padStart(26, "0")}`,
);

function createDatabase(filename: string, finish: string): void {
    const db = new Database(filename);
    try {
        db.exec(`CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT, time_updated INTEGER DEFAULT 0);
                 CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT, time_created INTEGER DEFAULT 0, time_updated INTEGER DEFAULT 0);`);
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
        ).run(
            "assistant",
            "session",
            100,
            JSON.stringify({ role: "assistant", finish, time: { completed: 100 } }),
        );
        db.prepare(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
        ).run("partless-user", "partless", 200, '{"role":"user"}');
        if (process.argv[2] === "normal-chunk" || growingChunks) {
            const message = db.prepare(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
            );
            const part = db.prepare(
                "INSERT INTO part (id, message_id, session_id, data) VALUES (?, ?, ?, ?)",
            );
            db.transaction(() => {
                for (const [i, id] of chunkIds.entries()) {
                    message.run(id, chunkSession, i + 1, '{"role":"user"}');
                    part.run(
                        `prt_${i}`,
                        id,
                        chunkSession,
                        JSON.stringify({ type: "text", text: id }),
                    );
                }
            })();
        }
        assert.notEqual(db.prepare("SELECT 1"), db.prepare("SELECT 1"));
    } finally {
        db.close();
    }
}

try {
    createDatabase(path, "stop");
    createDatabase(replacement, "tool-calls");
    const nativePrepare = Database.prototype.prepare;
    let preparations = 0;
    const preparingConnections = new Set<Database>();
    const nativeReads = new Map<Statement, (...args: unknown[]) => unknown>();
    Database.prototype.prepare = function (this: Database, sql: string) {
        preparations++;
        preparingConnections.add(this);
        const statement: Statement = nativePrepare.call(this, sql);
        nativeReads.set(statement, statement.get.bind(statement));
        return statement;
    } as Database["prepare"];
    const removal = process.argv[2];
    if (removal === "normal-chunk" || growingChunks) {
        const nativeClose = Database.prototype.close;
        let closes = 0;
        Database.prototype.close = function (this: Database) {
            closes++;
            return nativeClose.call(this);
        };
        try {
            assert.equal(isMidTurn(undefined, chunkSession), true);
            const reader = withReadOnlySessionDb((db) => db);
            assert.equal(preparations, 2);
            const sizes = growingChunks
                ? Array.from({ length: 70 }, (_, i) => 801 + i)
                : [800, 800];
            for (const [pass, size] of sizes.entries()) {
                const before: number = preparations;
                const ids = Object.freeze(chunkIds.slice(0, size));
                const times = getMessageTimesFromOpenCodeDb(chunkSession, ids);
                assert.deepEqual(times, new Map(ids.map((id, i) => [id, i + 1])));
                const page = readRawSessionMessagePageFromDb(reader, chunkSession, 0, size);
                assert.deepEqual(
                    page.map((message) => ({
                        id: message.id,
                        ordinal: message.ordinal,
                        parts: message.parts,
                    })),
                    ids.map((id, i) => ({
                        id,
                        ordinal: i + 1,
                        parts: [{ type: "text", text: id }],
                    })),
                );
                const afterChunk: number = preparations;
                assert.equal(isMidTurn(undefined, chunkSession), true);
                if (!growingChunks)
                    assert.equal(
                        preparations,
                        afterChunk,
                        "800-ID chunks must preserve warm mid-turn statements",
                    );
                if (!growingChunks && pass === 1)
                    assert.equal(
                        preparations,
                        before,
                        "normal chunk statements must also stay warm",
                    );
            }
            assert.deepEqual(
                { preparations, closes, connections: preparingConnections.size },
                { preparations: 5, closes: 0, connections: 1 },
                "chunk growth must preserve five warm statements on one native connection",
            );
        } finally {
            Database.prototype.close = nativeClose;
        }
    } else if (removal) {
        const reader = withReadOnlySessionDb((db) => db);
        const throws = removal.startsWith("throwing-");
        const sql = `${throws ? "SELECT json_extract(?, '$') AS value" : "SELECT ? AS value"}${
            removal.endsWith("sql") ? ` /*${"x".repeat(4096)}*/` : ""
        }`;
        const query = reader.prepare(sql);
        const value = removal.endsWith("bind") ? "{".repeat(64 * 1024 + 1) : throws ? "{" : "small";
        if (throws) assert.throws(() => query.get(value), /malformed JSON/);
        else assert.equal((query.get(value) as { value: string }).value, value);
        const nativeRead = [...nativeReads.values()].at(-1);
        assert.ok(nativeRead, "the probe must capture an actual native getter");

        if (removal === "finalize-failure") {
            const firstNative = [...nativeReads.keys()].at(-1) as Statement & {
                finalize?: () => void;
            };
            reader.prepare("SELECT ? AS value /* second */").get("second");
            const secondRead = [...nativeReads.values()].at(-1);
            assert.ok(secondRead);
            const finalize = firstNative.finalize?.bind(firstNative);
            if (finalize)
                firstNative.finalize = () => {
                    finalize();
                    throw new Error("injected finalize failure");
                };
            const nativeClose = Database.prototype.close;
            let closes = 0;
            Database.prototype.close = function (this: Database) {
                closes++;
                return nativeClose.call(this);
            };
            try {
                closeReadOnlySessionDb();
                assert.equal(closes, 1, "finalize failure must not bypass native close");
                assert.throws(() => secondRead("null"), "all cache entries must be finalized");
            } finally {
                Database.prototype.close = nativeClose;
            }
        } else {
            if (removal === "evicted") {
                for (let i = 0; i < 64; i++) reader.prepare(`SELECT ${i}`).get();
            }
            let liveAfterRemoval = false;
            try {
                nativeRead("null");
                liveAfterRemoval = true;
            } catch {}
            if (throws) assert.equal((query.get("null") as { value: unknown }).value, null);
            if (process.argv[3] === "replace") {
                renameSync(replacement, path);
                assert.equal(isMidTurn(undefined, "session"), true);
            } else {
                closeReadOnlySessionDb();
            }
            assert.throws(() => nativeRead("null"), `${removal}: native getter survives teardown`);
            assert.throws(
                () => query.get("null"),
                /closed/,
                "closed logical handles cannot reprepare",
            );
            assert.equal(liveAfterRemoval, false, `${removal}: native handle waits for GC`);
        }
    } else {
        assert.equal(isMidTurn(undefined, "session"), false);
        const coldPreparations = preparations;
        assert.ok(coldPreparations > 0);
        assert.equal(isMidTurn(undefined, "session"), false);
        assert.equal(preparations, coldPreparations, "warm read must reuse native statements");
        assert.equal(
            isMidTurn(undefined, "partless"),
            true,
            "a partless first user is real on both adapters",
        );

        const sql = "SELECT id FROM message WHERE session_id = ?";
        const oldDb = withReadOnlySessionDb((db) => db);
        const oldStatement = withReadOnlySessionDb((db) => db.prepare(sql));
        oldStatement.get("session");
        const oldNativeRead = [...nativeReads.values()].at(-1);
        assert.ok(oldNativeRead);
        assert.equal((oldNativeRead("session") as { id: string }).id, "assistant");
        assert.equal(
            withReadOnlySessionDb((db) => db.prepare(sql)),
            oldStatement,
        );
        assert.deepEqual(
            oldStatement.all("session").map((row) => ({ ...(row as object) })),
            [{ id: "assistant" }],
        );
        assert.throws(() => oldDb.prepare("DELETE FROM message").get(), /readonly/);
        const identity = statSync(path, { bigint: true });
        renameSync(replacement, path);
        assert.notEqual(statSync(path, { bigint: true }).ino, identity.ino);
        assert.equal(isMidTurn(undefined, "session"), true, "replacement must change the answer");
        assert.notEqual(
            withReadOnlySessionDb((db) => db),
            oldDb,
        );
        assert.throws(() => oldStatement.all("session"));
        assert.throws(() => oldNativeRead("session"));
        assert.throws(() => oldDb.prepare(sql));
        const freshStatement = withReadOnlySessionDb((db) => db.prepare(sql));
        freshStatement.get("session");
        const freshNativeRead = [...nativeReads.values()].at(-1);
        assert.ok(freshNativeRead);
        assert.equal((freshNativeRead("session") as { id: string }).id, "assistant");
        assert.notEqual(freshStatement, oldStatement);
        assert.equal(
            withReadOnlySessionDb((db) => db.prepare(sql)),
            freshStatement,
        );

        closeReadOnlySessionDb();
        assert.throws(() => freshStatement.all("session"));
        assert.throws(() => freshNativeRead("session"));
        assert.equal(isMidTurn(undefined, "session"), true);
        const reopened = withReadOnlySessionDb((db) => db.prepare(sql));
        assert.notEqual(reopened, freshStatement);

        createDatabase(replacement, "stop");
        process.env.OPENCODE_DB = replacement;
        assert.equal(isMidTurn(undefined, "session"), false);
        assert.throws(() => reopened.all("session"));

        const removed = withReadOnlySessionDb((db) => db.prepare(sql));
        unlinkSync(replacement);
        assert.equal(isMidTurn(undefined, "session"), false);
        assert.throws(() => removed.all("session"));
        writeFileSync(replacement, "not sqlite");
        assert.equal(isMidTurn(undefined, "session"), true);
        unlinkSync(replacement);
        createDatabase(replacement, "stop");
        assert.equal(isMidTurn(undefined, "session"), false);

        const boundedDb = withReadOnlySessionDb((db) => db);
        const first = boundedDb.prepare("SELECT 0 AS n");
        first.get();
        for (let i = 1; i <= 64; i++) boundedDb.prepare(`SELECT ${i} AS n`).get();
        assert.notEqual(boundedDb.prepare("SELECT 0 AS n"), first, "statement count is bounded");
        assert.equal(
            (first.get() as { n: number }).n,
            0,
            "eviction does not close borrowed statements",
        );
        const longSql = `SELECT 1 /*${"x".repeat(4096)}*/`;
        assert.notEqual(
            boundedDb.prepare(longSql),
            boundedDb.prepare(longSql),
            "SQL text is bounded",
        );
        const longQuery = boundedDb.prepare(longSql);
        for (let i = 0; i < 2; i++) {
            assert.equal(longQuery.all().length, 1);
            const nativeRead = [...nativeReads.values()].at(-1);
            assert.ok(nativeRead);
            assert.throws(() => nativeRead(), "oversized SQL must release its native handle");
        }
        const boundStatement = boundedDb.prepare("SELECT ? AS value");
        assert.equal((boundStatement.get("small") as { value: string }).value, "small");
        assert.equal(boundedDb.prepare("SELECT ? AS value"), boundStatement);
        const limitValue = "x".repeat(64 * 1024);
        assert.equal((boundStatement.get(limitValue) as { value: string }).value, limitValue);
        assert.equal(
            boundedDb.prepare("SELECT ? AS value"),
            boundStatement,
            "the binding limit is inclusive",
        );
        const largeValue = `${limitValue}x`;
        assert.equal((boundStatement.get(largeValue) as { value: string }).value, largeValue);
        assert.notEqual(
            boundedDb.prepare("SELECT ? AS value"),
            boundStatement,
            "bindings are bounded",
        );

        for (const value of [largeValue, "small again", largeValue, "last chunk"]) {
            assert.deepEqual(
                boundStatement.all(value).map((row) => ({ ...(row as object) })),
                [{ value }],
            );
            if (value === largeValue) {
                const nativeRead = [...nativeReads.values()].at(-1);
                assert.ok(nativeRead);
                assert.throws(
                    () => nativeRead("valid"),
                    "oversized chunk must release its native handle",
                );
            }
        }
        const arrayArgs = ["array bind"];
        const beforeArrayReads = preparations;
        assert.equal((boundStatement.get(arrayArgs) as { value: string }).value, arrayArgs[0]);
        assert.deepEqual(
            boundStatement.all(arrayArgs).map((row) => ({ ...(row as object) })),
            [{ value: arrayArgs[0] }],
        );
        assert.equal(
            preparations,
            beforeArrayReads,
            "bounded array binds reuse the warm statement",
        );

        const named = boundedDb.prepare("SELECT $value AS value");
        for (const value of ["first named", "second named"]) {
            assert.equal((named.get({ $value: value }) as { value: string }).value, value);
            const nativeRead = [...nativeReads.values()].at(-1);
            assert.ok(nativeRead);
            assert.throws(() => nativeRead({ $value: "valid" }), "named binds remain uncached");
        }
        assert.deepEqual(Object.keys(boundStatement).sort(), ["all", "get"]);
        for (const method of [
            "bind",
            "iterate",
            "values",
            "run",
            "raw",
            "safeIntegers",
            "setReadBigInts",
            "setReturnArrays",
            "columns",
        ]) {
            assert.equal(
                Reflect.get(boundStatement, method),
                undefined,
                `${method} must not bypass ownership`,
            );
        }

        const pressureReads: Array<(...args: unknown[]) => unknown> = [];
        for (let i = 0; i < 128; i++) {
            const query = boundedDb.prepare(`SELECT ? AS value /* pressure ${i} */`);
            query.get("x".repeat(4096));
            const nativeRead = [...nativeReads.values()].at(-1);
            assert.ok(nativeRead);
            pressureReads.push(nativeRead);
            let live = 0;
            for (const read of pressureReads) {
                try {
                    read("probe");
                    live++;
                } catch {}
            }
            assert.ok(live <= 64, `native handles exceed cache capacity: ${live}`);
        }
        closeReadOnlySessionDb();
        for (const read of pressureReads) assert.throws(() => read("probe"));
    }
    Database.prototype.prepare = nativePrepare;
    console.log(`session-db cache contract passed: ${detectSqliteRuntime()}`);
} finally {
    closeReadOnlySessionDb();
    if (originalOverride === undefined) delete process.env.OPENCODE_DB;
    else process.env.OPENCODE_DB = originalOverride;
    rmSync(dir, { recursive: true, force: true });
}
