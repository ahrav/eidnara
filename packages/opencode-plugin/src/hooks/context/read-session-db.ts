import { existsSync, statSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "../../shared/data-path";
import { log } from "../../shared/logger";
import {
    Database,
    type SqliteReader,
    type SqliteReadStatement,
    type Statement,
} from "../../shared/sqlite";
import { closeQuietly, jsonField } from "../../shared/sqlite-helpers";
import { isMachineAuthoredPart, isMeaningfulUserText } from "./read-session-formatting";

interface AssistantMidTurnRow {
    id?: string;
    finish?: string | null;
    timeCreated?: number;
    timeCompleted?: number | null;
}

interface PartDataRow {
    data?: unknown;
}

function getOpenCodeDbPath(): string {
    // `OPENCODE_DB` is OpenCode's own override, so the plugin reads the database OpenCode selected.
    const override = process.env.OPENCODE_DB;
    if (typeof override === "string" && override.length > 0) return override;
    return join(getDataDir(), "opencode", "opencode.db");
}

export function refreshOpenCodeDbPresence(): boolean {
    const exists = existsSync(getOpenCodeDbPath());
    if (!exists) closeCachedReadOnlyDb();
    return exists;
}

const MAX_CACHED_STATEMENTS = 64;
const MAX_CACHED_SQL_UNITS = 4096;
const MAX_CACHED_BIND_BYTES = 128 * 1024;

function canRetainBindings(args: unknown[]): boolean {
    let bytes = 0;
    if (args.length > MAX_CACHED_BIND_BYTES / 8) return false;
    for (const arg of args) {
        if (typeof arg === "string") bytes += arg.length * 2;
        else if (typeof arg === "number" || typeof arg === "bigint") bytes += 8;
        else if (ArrayBuffer.isView(arg)) bytes += arg.byteLength;
        else if (arg !== null) return false;
        if (bytes > MAX_CACHED_BIND_BYTES) return false;
    }
    return true;
}

type NativeReadStatement = Statement & { finalize?: () => void };

class ReadOnlySessionDb implements SqliteReader {
    #statements = new Map<string, { native: NativeReadStatement; query: SqliteReadStatement }>();
    #db: Database | null = null;
    #closed = false;

    constructor(
        readonly path: string,
        readonly dev: bigint,
        readonly ino: bigint,
    ) {
        this.#connection();
    }

    get closed(): boolean {
        return this.#closed;
    }

    #connection(): Database {
        if (this.#closed) throw new Error("OpenCode read-only database is closed");
        if (this.#db) return this.#db;
        const before = statSync(this.path, { bigint: true });
        if (before.dev !== this.dev || before.ino !== this.ino) {
            throw new Error("OpenCode database identity changed before opening");
        }
        const db = new Database(this.path, { readonly: true });
        try {
            const after = statSync(this.path, { bigint: true });
            if (after.dev !== this.dev || after.ino !== this.ino) {
                throw new Error("OpenCode database changed while opening the read-only connection");
            }
            this.#db = db;
            return db;
        } catch (error) {
            closeQuietly(db);
            throw error;
        }
    }

    prepare(sql: string): SqliteReadStatement {
        if (this.#closed) throw new Error("OpenCode read-only database is closed");
        const cached = this.#statements.get(sql);
        if (cached) return cached.query;
        // Handles keep SQL, not native statements, so a chunk loop can resume after eviction.
        const query: SqliteReadStatement = Object.freeze({
            get: (...args: unknown[]) => this.#execute(sql, query, "get", args),
            all: (...args: unknown[]) => this.#execute(sql, query, "all", args) as unknown[],
        });
        return query;
    }

    #execute(
        sql: string,
        query: SqliteReadStatement,
        method: "get" | "all",
        args: unknown[],
    ): unknown {
        if (this.#closed) throw new Error("OpenCode read-only database is closed");
        const bindings = args.length === 1 && Array.isArray(args[0]) ? args[0] : args;
        const retain = sql.length <= MAX_CACHED_SQL_UNITS && canRetainBindings(bindings);
        let entry = this.#statements.get(sql);
        if (entry && !retain) {
            this.#statements.delete(sql);
            this.#finalize(entry.native);
            entry = undefined;
        }
        if (!entry) {
            if (retain && this.#statements.size === MAX_CACHED_STATEMENTS) {
                const oldest = this.#statements.entries().next().value;
                if (oldest) {
                    this.#statements.delete(oldest[0]);
                    this.#finalize(oldest[1].native);
                }
            }
            entry = { native: this.#connection().prepare(sql), query };
            if (retain) this.#statements.set(sql, entry);
        }
        try {
            return entry.native[method](...bindings);
        } finally {
            if (!retain) this.#finalize(entry.native);
        }
    }

    #finalize(statement: NativeReadStatement): void {
        try {
            if (statement.finalize) statement.finalize();
            // Node has no statement finalizer; closing its connection releases all native handles.
            else this.#closeConnection();
        } catch (error) {
            this.close();
            throw error;
        }
    }

    #closeConnection(): void {
        const db = this.#db;
        this.#db = null;
        let failure: unknown;
        try {
            for (const entry of this.#statements.values()) {
                try {
                    entry.native.finalize?.();
                } catch (error) {
                    failure ??= error;
                }
            }
        } finally {
            this.#statements.clear();
            db?.close();
        }
        if (failure !== undefined) throw failure;
    }

    close(): void {
        this.#closed = true;
        this.#closeConnection();
    }
}

let cachedReadOnlyDb: ReadOnlySessionDb | null = null;

function closeCachedReadOnlyDb(): void {
    const db = cachedReadOnlyDb;
    cachedReadOnlyDb = null;
    try {
        db?.close();
    } catch (error) {
        log("[eidnara] failed to close cached OpenCode read-only DB:", error);
    }
}

function getReadOnlySessionDb(): SqliteReader {
    const dbPath = getOpenCodeDbPath();
    try {
        const { dev, ino } = statSync(dbPath, { bigint: true });
        if (
            cachedReadOnlyDb?.path === dbPath &&
            cachedReadOnlyDb.dev === dev &&
            cachedReadOnlyDb.ino === ino &&
            !cachedReadOnlyDb.closed
        ) {
            return cachedReadOnlyDb;
        }
        closeCachedReadOnlyDb();
        cachedReadOnlyDb = new ReadOnlySessionDb(dbPath, dev, ino);
        return cachedReadOnlyDb;
    } catch (error) {
        closeCachedReadOnlyDb();
        throw error;
    }
}

export function withReadOnlySessionDb<T>(fn: (db: SqliteReader) => T): T {
    return fn(getReadOnlySessionDb());
}

export function closeReadOnlySessionDb(): void {
    closeCachedReadOnlyDb();
}

/** Treat errors reading an existing database as mid-turn; a missing database is idle. */
export function isMidTurn(_deps: unknown, sessionId: string): boolean {
    if (!refreshOpenCodeDbPresence()) return false;
    try {
        return withReadOnlySessionDb((db) => isMidTurnFromOpenCodeDb(db, sessionId));
    } catch (error) {
        log("[eidnara] failed to inspect OpenCode mid-turn state; treating as mid-turn:", error);
        return true;
    }
}

export function isMidTurnFromOpenCodeDb(db: SqliteReader, sessionId: string): boolean {
    // `(time_created, id)` is the session ordering `read-session-raw.ts` uses; the id tiebreak
    // resolves two assistant rows that share a millisecond.
    // A compaction summary is written mid-turn and would otherwise hide the `tool-calls`
    // assistant that is still the active fence.
    const latestAssistant = db
        .prepare(
            `SELECT id,
                    json_extract(data, '$.finish') as finish,
                    json_extract(data, '$.time.completed') as timeCompleted,
                    time_created as timeCreated
             FROM message
             WHERE session_id = ?
               AND ${jsonField("data", "$.role")} = 'assistant'
               AND NOT (
                 COALESCE(${jsonField("data", "$.summary")}, 0) = 1
                 AND COALESCE(${jsonField("data", "$.finish")}, '') = 'stop'
               )
             ORDER BY time_created DESC, id DESC
             LIMIT 1`,
        )
        .get(sessionId) as AssistantMidTurnRow | null;

    // The fallback tuple lets a first user prompt count before an assistant exists.
    if (
        hasNewerRealUserMessage(
            db,
            sessionId,
            latestAssistant?.id ?? "",
            latestAssistant?.timeCreated ?? -1,
        )
    ) {
        return true;
    }
    if (typeof latestAssistant?.id !== "string") return false;
    // A missing `time.completed` marks an assistant message that is still being produced.
    if (typeof latestAssistant.timeCompleted !== "number") return true;
    if (latestAssistant.finish === "tool-calls") return true;

    // Only completed assistants require part classification.
    const partRows = db
        .prepare("SELECT data FROM part WHERE session_id = ? AND message_id = ?")
        .all(sessionId, latestAssistant.id) as PartDataRow[];

    // A synthetic tool part is the daemon's own bookkeeping, not a local call still in flight.
    return partRows.some((row) => {
        const part = parsePart(row);
        return (
            part !== null &&
            part.type === "tool" &&
            !isProviderExecuted(part) &&
            !isMachineAuthoredPart(part)
        );
    });
}

/** Accepts `providerExecuted` at `metadata.providerExecuted` (the persisted OpenCode shape) or at top level. */
function isProviderExecuted(part: Record<string, unknown>): boolean {
    if (part.providerExecuted === true) return true;
    const metadata = part.metadata;
    return (
        metadata !== null &&
        typeof metadata === "object" &&
        (metadata as Record<string, unknown>).providerExecuted === true
    );
}

function hasNewerRealUserMessage(
    db: SqliteReader,
    sessionId: string,
    latestAssistantId: string,
    latestAssistantTimeCreated: number,
): boolean {
    // "Newer" is the `(time_created, id)` tuple ordering, so a user row that shares the
    // assistant's millisecond still counts when its id sorts after the assistant's.
    // A `compaction` part excludes the whole message.
    const candidates = db
        .prepare(
            `SELECT m.id, p.data, p.message_id IS NOT NULL AS hasPart
             FROM message m
             LEFT JOIN part p ON p.session_id = m.session_id AND p.message_id = m.id
             WHERE m.session_id = ?
               AND (m.time_created > ? OR (m.time_created = ? AND m.id > ?))
               AND ${jsonField("m.data", "$.role")} = 'user'
               AND NOT EXISTS (
                 SELECT 1 FROM part c
                 WHERE c.session_id = m.session_id AND c.message_id = m.id
                   AND ${jsonField("c.data", "$.type")} = 'compaction'
               )
             ORDER BY m.time_created ASC, m.id ASC`,
        )
        .all(
            sessionId,
            latestAssistantTimeCreated,
            latestAssistantTimeCreated,
            latestAssistantId,
        ) as (PartDataRow & { id?: unknown; hasPart: number })[];

    return candidates.some((row) => {
        if (typeof row.id !== "string") return false;
        // A partless user message counts as real; NULL data on an existing part does not.
        if (row.hasPart === 0) return true;
        const part = parsePart(row);
        return part !== null && isRealUserPart(part);
    });
}

/**
 * Parts with `synthetic`, `ignored`, or `metadata.marker.kind` do not count. Text parts count only
 * when `isMeaningfulUserText` returns true; other typed, unflagged parts count.
 */
function isRealUserPart(part: Record<string, unknown>): boolean {
    if (typeof part.type !== "string") return false;
    if (isMachineAuthoredPart(part)) return false;
    if (part.type === "text") {
        return typeof part.text === "string" && isMeaningfulUserText(part.text);
    }
    return true;
}

function parsePart(row: PartDataRow): Record<string, unknown> | null {
    if (typeof row.data !== "string" || row.data.length === 0) return null;
    try {
        const parsed: unknown = JSON.parse(row.data);
        return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
            ? (parsed as Record<string, unknown>)
            : null;
    } catch {
        return null;
    }
}

interface AssistantModelRow {
    providerID?: string;
    modelID?: string;
}

/**
 *
 */
interface MessageTimeRow {
    id?: string;
    time_created?: number;
}

// `node:sqlite` caps a statement at 32,766 bound parameters; 800 ids per `IN (...)` stays far below
// that and matches the chunk size `read-session-raw.ts` uses for part lookups.
const MESSAGE_ID_CHUNK = 800;

/**
 *
 * `<session-history>`.
 */
export function getMessageTimesFromOpenCodeDb(
    sessionId: string,
    messageIds: readonly string[],
): Map<string, number> {
    const result = new Map<string, number>();
    if (messageIds.length === 0) return result;

    try {
        withReadOnlySessionDb((db) => {
            // NULL padding fixes the SQL shape without adding IDs to the lookup.
            const placeholders = Array(MESSAGE_ID_CHUNK).fill("?").join(",");
            const selectTimes = db.prepare(
                `SELECT id, time_created FROM message WHERE session_id = ? AND id IN (${placeholders})`,
            );
            for (let start = 0; start < messageIds.length; start += MESSAGE_ID_CHUNK) {
                const chunk: (string | null)[] = messageIds.slice(start, start + MESSAGE_ID_CHUNK);
                while (chunk.length < MESSAGE_ID_CHUNK) chunk.push(null);
                const rows = selectTimes.all(sessionId, ...chunk) as MessageTimeRow[];
                for (const row of rows) {
                    if (typeof row.id === "string" && typeof row.time_created === "number") {
                        result.set(row.id, row.time_created);
                    }
                }
            }
        });
    } catch (error) {
        log("[eidnara] failed to resolve message times from OpenCode DB:", error);
    }

    return result;
}

/** The newest assistant row with a model, regardless of usage tokens; `messageID` is that row's id. */
export function findLastAssistantModelFromOpenCodeDb(
    sessionId: string,
): { messageID: string; providerID: string; modelID: string; agent?: string } | null {
    if (!refreshOpenCodeDbPresence()) return null;
    try {
        return withReadOnlySessionDb((db) => {
            const row = db
                .prepare(
                    `SELECT id as messageID,
                            json_extract(data, '$.providerID') as providerID,
                            json_extract(data, '$.modelID') as modelID,
                            json_extract(data, '$.agent') as agent
                     FROM message
                     WHERE session_id = ?
                       AND ${jsonField("data", "$.role")} = 'assistant'
                       AND ${jsonField("data", "$.providerID")} IS NOT NULL
                       AND ${jsonField("data", "$.modelID")} IS NOT NULL
                     ORDER BY time_created DESC, id DESC
                     LIMIT 1`,
                )
                .get(sessionId) as
                | (AssistantModelRow & { messageID?: unknown; agent?: string | null })
                | null;
            if (
                !row ||
                typeof row.messageID !== "string" ||
                typeof row.providerID !== "string" ||
                typeof row.modelID !== "string"
            ) {
                return null;
            }
            const agent =
                typeof row.agent === "string" && row.agent.length > 0 ? row.agent : undefined;
            return {
                messageID: row.messageID,
                providerID: row.providerID,
                modelID: row.modelID,
                ...(agent ? { agent } : {}),
            };
        });
    } catch (error) {
        log("[eidnara] failed to recover live model from OpenCode DB:", error);
        return null;
    }
}

export interface PersistedAssistantUsage {
    messageID: string;
    providerID: string;
    modelID: string;
    /** Prompt tokens: `input + cache.read + cache.write`. */
    inputTokens: number;
    /** `time.completed` when the row has one, else `time_created`. */
    respondedAt: number;
}

/** Whether the session holds a compaction summary row, the boundary `findLastAssistantUsageFromOpenCodeDb` skips back to. */
export function sessionHasCompactionSummaryInOpenCodeDb(sessionId: string): boolean {
    if (!refreshOpenCodeDbPresence()) return false;
    try {
        return withReadOnlySessionDb((db) => {
            const row = db
                .prepare(
                    `SELECT 1 AS present
                     FROM message
                     WHERE session_id = ?
                       AND COALESCE(${jsonField("data", "$.summary")}, 0) = 1
                     LIMIT 1`,
                )
                .get(sessionId);
            return row !== null && row !== undefined;
        });
    } catch (error) {
        log("[eidnara] failed to probe compaction summary in OpenCode DB:", error);
        return false;
    }
}

/** Recovers persisted assistant usage after a restart or idle eviction. Rows at or before the newest compaction summary in `(time_created, id)` order are skipped, so a compacted session reports no usage until a post-compaction response lands. */
export function findLastAssistantUsageFromOpenCodeDb(
    sessionId: string,
): PersistedAssistantUsage | null {
    if (!refreshOpenCodeDbPresence()) return null;
    const promptTokens = `COALESCE(${jsonField("m.data", "$.tokens.input")}, 0)
                              + COALESCE(${jsonField("m.data", "$.tokens.cache.read")}, 0)
                              + COALESCE(${jsonField("m.data", "$.tokens.cache.write")}, 0)`;
    try {
        return withReadOnlySessionDb((db) => {
            const row = db
                .prepare(
                    `WITH boundary AS (
                       SELECT time_created, id
                       FROM message
                       WHERE session_id = ?1
                         AND COALESCE(${jsonField("data", "$.summary")}, 0) = 1
                       ORDER BY time_created DESC, id DESC
                       LIMIT 1
                     )
                     SELECT m.id,
                            ${jsonField("m.data", "$.providerID")} as providerID,
                            ${jsonField("m.data", "$.modelID")} as modelID,
                            ${promptTokens} as inputTokens,
                            ${jsonField("m.data", "$.time.completed")} as completedAt,
                            m.time_created as timeCreated
                     FROM message m
                     LEFT JOIN boundary b
                     WHERE m.session_id = ?1
                       AND ${jsonField("m.data", "$.role")} = 'assistant'
                       AND COALESCE(${jsonField("m.data", "$.summary")}, 0) <> 1
                       AND (
                         b.id IS NULL
                         OR m.time_created > b.time_created
                         OR (m.time_created = b.time_created AND m.id > b.id)
                       )
                       AND ${jsonField("m.data", "$.providerID")} IS NOT NULL
                       AND ${jsonField("m.data", "$.modelID")} IS NOT NULL
                       AND ${promptTokens} > 0
                     ORDER BY m.time_created DESC, m.id DESC
                     LIMIT 1`,
                )
                .get(sessionId) as {
                id?: unknown;
                providerID?: unknown;
                modelID?: unknown;
                inputTokens?: unknown;
                completedAt?: unknown;
                timeCreated?: unknown;
            } | null;
            if (
                !row ||
                typeof row.id !== "string" ||
                typeof row.providerID !== "string" ||
                typeof row.modelID !== "string" ||
                typeof row.inputTokens !== "number" ||
                row.inputTokens <= 0
            ) {
                return null;
            }
            const respondedAt =
                typeof row.completedAt === "number"
                    ? row.completedAt
                    : typeof row.timeCreated === "number"
                      ? row.timeCreated
                      : 0;
            return {
                messageID: row.id,
                providerID: row.providerID,
                modelID: row.modelID,
                inputTokens: row.inputTokens,
                respondedAt,
            };
        });
    } catch (error) {
        log("[eidnara] failed to recover assistant usage from OpenCode DB:", error);
        return null;
    }
}
