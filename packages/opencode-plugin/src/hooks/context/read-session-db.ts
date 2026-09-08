import { existsSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "../../shared/data-path";
import { log } from "../../shared/logger";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";

interface AssistantMidTurnRow {
    id?: string;
    finish?: string | null;
    timeCreated?: number;
}

interface ExistenceRow {
    one?: number;
}

interface PartDataRow {
    data?: string | null;
}

function getOpenCodeDbPath(): string {
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

export function isMidTurn(_deps: unknown, sessionId: string): boolean {
    try {
        return withReadOnlySessionDb((db) => isMidTurnFromOpenCodeDb(db, sessionId));
    } catch (error) {
        log("[eidnara] failed to inspect OpenCode mid-turn state:", error);
        return false;
    }
}

export function isMidTurnFromOpenCodeDb(db: Database, sessionId: string): boolean {
    // `(time_created, id)` is the session ordering `read-session-raw.ts` uses; the id tiebreak
    // resolves two assistant rows that share a millisecond.
    const latestAssistant = db
        .prepare(
            `SELECT id,
                    json_extract(data, '$.finish') as finish,
                    time_created as timeCreated
             FROM message
             WHERE session_id = ?
               AND json_extract(data, '$.role') = 'assistant'
             ORDER BY time_created DESC, id DESC
             LIMIT 1`,
        )
        .get(sessionId) as AssistantMidTurnRow | null;

    if (typeof latestAssistant?.id !== "string") return false;
    if (hasNewerRealUserMessage(db, sessionId, latestAssistant.id, latestAssistant.timeCreated)) {
        return false;
    }
    if (latestAssistant.finish === "tool-calls") return true;

    const partRows = db
        .prepare("SELECT data FROM part WHERE session_id = ? AND message_id = ?")
        .all(sessionId, latestAssistant.id) as PartDataRow[];

    return partRows.some((row) => {
        if (typeof row.data !== "string" || row.data.length === 0) return false;
        try {
            const part = JSON.parse(row.data) as Record<string, unknown>;
            return part.type === "tool" && !isProviderExecuted(part);
        } catch {
            return false;
        }
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
    db: Database,
    sessionId: string,
    latestAssistantId: string,
    latestAssistantTimeCreated: unknown,
): boolean {
    if (typeof latestAssistantTimeCreated !== "number") return false;
    // "Newer" is the `(time_created, id)` tuple ordering, so a user row that shares the
    // assistant's millisecond still counts when its id sorts after the assistant's.
    const row = db
        .prepare(
            `SELECT 1 as one
             FROM message m
             WHERE m.session_id = ?
               AND (m.time_created > ? OR (m.time_created = ? AND m.id > ?))
               AND json_extract(m.data, '$.role') = 'user'
               AND NOT EXISTS (
                 SELECT 1 FROM part p
                 WHERE p.message_id = m.id
                   AND json_extract(p.data, '$.type') = 'compaction'
               )
               AND NOT (
                 EXISTS (SELECT 1 FROM part p WHERE p.message_id = m.id)
                 AND NOT EXISTS (
                   SELECT 1 FROM part p
                   WHERE p.message_id = m.id
                     AND COALESCE(json_extract(p.data, '$.synthetic'), 0) NOT IN (1, 'true')
                     AND json_extract(p.data, '$.metadata.marker.kind') IS NULL
                     AND COALESCE(json_extract(p.data, '$.ignored'), 0) NOT IN (1, 'true')
                 )
               )
             LIMIT 1`,
        )
        .get(
            sessionId,
            latestAssistantTimeCreated,
            latestAssistantTimeCreated,
            latestAssistantId,
        ) as ExistenceRow | null;
    // A `compaction` part excludes the whole message before real-part filtering: the summary-prompt
    // text part beside it is unflagged and would otherwise satisfy the per-part predicate.
    // Parts with synthetic=true, metadata.marker.kind, or an ignored flag do not make a user message real.
    // A user message with at least one non-synthetic, unmarked, non-ignored part counts as real.
    // A partless user message counts as real.
    return row?.one === 1;
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
            const placeholders = messageIds.map(() => "?").join(",");
            const rows = db
                .prepare(
                    `SELECT id, time_created FROM message WHERE session_id = ? AND id IN (${placeholders})`,
                )
                .all(sessionId, ...messageIds) as MessageTimeRow[];
            for (const row of rows) {
                if (typeof row.id === "string" && typeof row.time_created === "number") {
                    result.set(row.id, row.time_created);
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
                       AND json_extract(data, '$.role') = 'assistant'
                       AND json_extract(data, '$.providerID') IS NOT NULL
                       AND json_extract(data, '$.modelID') IS NOT NULL
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
