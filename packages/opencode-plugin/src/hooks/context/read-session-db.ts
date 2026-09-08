import { existsSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "../../shared/data-path";
import { log } from "../../shared/logger";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import { isMachineAuthoredPart, isMeaningfulUserText } from "./read-session-formatting";

interface AssistantMidTurnRow {
    id?: string;
    finish?: string | null;
    timeCreated?: number;
    timeCompleted?: number | null;
}

interface MessageIdRow {
    id?: string;
}

interface PartDataRow {
    data?: string | null;
}

function getOpenCodeDbPath(): string {
    // `OPENCODE_DB` is OpenCode's own override, so the plugin reads the database OpenCode selected.
    const override = process.env.OPENCODE_DB;
    if (typeof override === "string" && override.length > 0) return override;
    return join(getDataDir(), "opencode", "opencode.db");
}

/**
 */
export function openCodeDbExists(): boolean {
    return existsSync(getOpenCodeDbPath());
}

let cachedReadOnlyDb: { path: string; db: Database } | null = null;

function closeCachedReadOnlyDb(): void {
    if (!cachedReadOnlyDb) {
        return;
    }

    try {
        closeQuietly(cachedReadOnlyDb.db);
    } catch (error) {
        log("[eidnara] failed to close cached OpenCode read-only DB:", error);
    } finally {
        cachedReadOnlyDb = null;
    }
}

function getReadOnlySessionDb(): Database {
    const dbPath = getOpenCodeDbPath();
    if (cachedReadOnlyDb?.path === dbPath) {
        return cachedReadOnlyDb.db;
    }

    closeCachedReadOnlyDb();
    const db = new Database(dbPath, { readonly: true });
    cachedReadOnlyDb = { path: dbPath, db };
    return db;
}

export function withReadOnlySessionDb<T>(fn: (db: Database) => T): T {
    return fn(getReadOnlySessionDb());
}

export function closeReadOnlySessionDb(): void {
    closeCachedReadOnlyDb();
}

/**
 * Builds a `json_extract` that yields NULL for a malformed `column` instead of raising `malformed JSON`.
 * `CASE` evaluates only the taken branch, so the extract never runs on an invalid document; an `AND`
 * guard has no such ordering guarantee. `column` and `path` are code literals, never caller input.
 */
function jsonField(column: string, path: string): string {
    return `CASE WHEN json_valid(${column}) = 1 THEN json_extract(${column}, '${path}') END`;
}

/** Treat errors reading an existing database as mid-turn; a missing database is idle. */
export function isMidTurn(_deps: unknown, sessionId: string): boolean {
    if (!openCodeDbExists()) return false;
    try {
        return withReadOnlySessionDb((db) => isMidTurnFromOpenCodeDb(db, sessionId));
    } catch (error) {
        log("[eidnara] failed to inspect OpenCode mid-turn state; treating as mid-turn:", error);
        return true;
    }
}

export function isMidTurnFromOpenCodeDb(db: Database, sessionId: string): boolean {
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

    // A real user row newer than the latest assistant is a turn whose assistant row does not exist
    // yet, including the first prompt of a session with no assistant row at all.
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

/** Callers pass `""` and `-1` when the session has no assistant row; every user row is then newer. */
function hasNewerRealUserMessage(
    db: Database,
    sessionId: string,
    latestAssistantId: string,
    latestAssistantTimeCreated: number,
): boolean {
    // "Newer" is the `(time_created, id)` tuple ordering, so a user row that shares the
    // assistant's millisecond still counts when its id sorts after the assistant's.
    // A `compaction` part excludes the whole message.
    const candidates = db
        .prepare(
            `SELECT m.id
             FROM message m
             WHERE m.session_id = ?
               AND (m.time_created > ? OR (m.time_created = ? AND m.id > ?))
               AND ${jsonField("m.data", "$.role")} = 'user'
               AND NOT EXISTS (
                 SELECT 1 FROM part p
                 WHERE p.message_id = m.id
                   AND ${jsonField("p.data", "$.type")} = 'compaction'
               )
             ORDER BY m.time_created ASC, m.id ASC`,
        )
        .all(
            sessionId,
            latestAssistantTimeCreated,
            latestAssistantTimeCreated,
            latestAssistantId,
        ) as MessageIdRow[];

    const selectParts = db.prepare("SELECT data FROM part WHERE message_id = ?");
    for (const candidate of candidates) {
        if (typeof candidate.id !== "string") continue;
        const partRows = selectParts.all(candidate.id) as PartDataRow[];
        if (isRealUserMessage(partRows)) return true;
    }
    return false;
}

/**
 * A partless user message counts as real. Otherwise at least one part must be real; a malformed
 * part is not evidence of a real turn.
 */
function isRealUserMessage(partRows: PartDataRow[]): boolean {
    if (partRows.length === 0) return true;
    return partRows.some((row) => {
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
            for (let start = 0; start < messageIds.length; start += MESSAGE_ID_CHUNK) {
                const chunk = messageIds.slice(start, start + MESSAGE_ID_CHUNK);
                const placeholders = chunk.map(() => "?").join(",");
                const rows = db
                    .prepare(
                        `SELECT id, time_created FROM message WHERE session_id = ? AND id IN (${placeholders})`,
                    )
                    .all(sessionId, ...chunk) as MessageTimeRow[];
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

export function findLastAssistantModelFromOpenCodeDb(
    sessionId: string,
): { providerID: string; modelID: string; agent?: string } | null {
    try {
        return withReadOnlySessionDb((db) => {
            const row = db
                .prepare(
                    `SELECT json_extract(data, '$.providerID') as providerID,
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
                .get(sessionId) as (AssistantModelRow & { agent?: string | null }) | null;
            if (!row || typeof row.providerID !== "string" || typeof row.modelID !== "string") {
                return null;
            }
            const agent =
                typeof row.agent === "string" && row.agent.length > 0 ? row.agent : undefined;
            return {
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
