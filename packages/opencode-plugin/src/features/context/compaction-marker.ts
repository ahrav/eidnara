/**
 *
 * `injectCompactionMarker` injects compaction boundaries into OpenCode's SQLite database.
 * `filterCompacted` stops at the historian boundary after injection.
 *
 *
 * A marker contains a `compaction` part on its boundary user message, tagged with an `eidnara` key.
 * A marker contains a summary assistant message whose `parentID` equals the boundary user message's `id`.
 * A marker contains a text part with a static placeholder on its summary message.
 *
 * The marker exists solely to make `filterCompacted` stop at the boundary.
 *
 * The search iterates from newest to oldest.
 * The search stops at the first user message with a `compaction` part and a qualifying summary response.
 * The user message must contain a part with `type: "compaction"`.
 * The user message must have a summary assistant response with `summary: true` and `finish: "stop"`.
 * The summary response's `parentID` must equal the user message's `id`.
 */

import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "../../shared/data-path";
import { log } from "../../shared/logger";
import {
    collectSqliteRuntimeGateInput,
    Database,
    evaluateSqliteRuntimeGate,
    runImmediate,
    type SqliteRuntimeGateResult,
} from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";

const BASE62_CHARS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ID_PREFIX_HEX_LENGTH = 12;
const ID_SUFFIX_LENGTH = 14;
const ID_PREFIX_MASK = (1n << BigInt(ID_PREFIX_HEX_LENGTH * 4)) - 1n;

/**
 * `EIDNARA_PROVIDER_ID` identifies summaries injected by this module; ownership queries match this value.
 * OpenCode-native summaries carry a real provider ID and never match it.
 */
export const EIDNARA_PROVIDER_ID = "eidnara";

function deterministicBase62(seed: string, length: number): string {
    let value = BigInt(`0x${createHash("sha256").update(seed).digest("hex")}`);
    const chars = Array<string>(length);
    for (let index = length - 1; index >= 0; index -= 1) {
        chars[index] = BASE62_CHARS[Number(value % 62n)];
        value /= 62n;
    }
    return chars.join("");
}

/**
 * Format: `prefix_[12-hex-chars][14-deterministic-base62]`.
 */
function generateId(
    prefix: string,
    timestampMs: number,
    counter: bigint,
    identity: string,
): string {
    const encoded =
        (BigInt(Math.max(0, Math.floor(timestampMs))) * 0x1000n + counter) & ID_PREFIX_MASK;
    const hex = encoded.toString(16).padStart(ID_PREFIX_HEX_LENGTH, "0");
    return `${prefix}_${hex}${deterministicBase62(`${prefix}\0${identity}`, ID_SUFFIX_LENGTH)}`;
}

export function generateMessageId(timestampMs: number, counter = 0n, identity = ""): string {
    return generateId("msg", timestampMs, counter, identity);
}

export function generatePartId(timestampMs: number, counter = 0n, identity = ""): string {
    return generateId("prt", timestampMs, counter, identity);
}

/**
 * Ownership lives in the part payload because `/fork` assigns new primary keys to copied rows.
 * A captured native row (`crates/daemon/testdata/codec/opencode-golden.json`) carries only
 * `type` and `auto`, so the extra key is the only durable signature.
 */
const EIDNARA_COMPACTION_PART_DATA = JSON.stringify({
    type: "compaction",
    auto: true,
    [EIDNARA_PROVIDER_ID]: true,
});

function isEidnaraCompactionPartData(data: unknown): boolean {
    if (typeof data !== "object" || data === null || Array.isArray(data)) return false;
    const record = data as Record<string, unknown>;
    return record.type === "compaction" && record[EIDNARA_PROVIDER_ID] === true;
}

export function getOpenCodeDbPath(): string {
    return join(getDataDir(), "opencode", "opencode.db");
}

let cachedWriteDb: { path: string; db: Database } | null = null;

let cachedRuntimeGate: SqliteRuntimeGateResult | null = null;

/** The verdict depends only on the running engine, so it is evaluated once per process. */
function assertSqliteRuntimeGate(): void {
    cachedRuntimeGate ??= evaluateSqliteRuntimeGate(collectSqliteRuntimeGateInput());
    if (!cachedRuntimeGate.ok) {
        throw new Error(
            `SQLite runtime rejected for opencode.db writes: ${cachedRuntimeGate.reasons.join("; ")}`,
        );
    }
}

// `REQUIRED_MESSAGE_COLUMNS` and `REQUIRED_PART_COLUMNS` list every column used by `injectCompactionMarker` INSERT statements.
// The schema probe detects missing required columns before marker writes.
// The pre-write probe prevents a missing column from causing partial marker state.
const REQUIRED_MESSAGE_COLUMNS = ["id", "session_id", "time_created", "time_updated", "data"];
const REQUIRED_PART_COLUMNS = [
    "id",
    "message_id",
    "session_id",
    "time_created",
    "time_updated",
    "data",
];

function tableColumnSet(db: Database, table: string): Set<string> {
    if (!/^[a-z][a-z0-9_]*$/.test(table)) {
        throw new Error(`Unsafe schema identifier: ${table}`);
    }
    const rows = db.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name?: string }>;
    return new Set(rows.map((row) => row.name ?? "").filter((name) => name.length > 0));
}

let cachedSchemaCompatible: { path: string; compatible: boolean } | null = null;

/**
 * Reject marker injection unless both tables contain every column used by its INSERTs.
 */
function isOpenCodeSchemaCompatible(db: Database, dbPath: string): boolean {
    if (cachedSchemaCompatible?.path === dbPath) {
        return cachedSchemaCompatible.compatible;
    }

    try {
        const messageCols = tableColumnSet(db, "message");
        const partCols = tableColumnSet(db, "part");

        const missingMessage = REQUIRED_MESSAGE_COLUMNS.filter((c) => !messageCols.has(c));
        const missingPart = REQUIRED_PART_COLUMNS.filter((c) => !partCols.has(c));

        if (missingMessage.length > 0 || missingPart.length > 0) {
            log(
                `[eidnara] compaction-marker: OpenCode DB schema missing required columns ` +
                    `(message: [${missingMessage.join(", ")}], part: [${missingPart.join(", ")}]). ` +
                    `Marker injection disabled for this process. ` +
                    `This usually means OpenCode was updated and Eidnara is out of date.`,
            );
            cachedSchemaCompatible = { path: dbPath, compatible: false };
            return false;
        }

        cachedSchemaCompatible = { path: dbPath, compatible: true };
        return true;
    } catch (error) {
        // A probe exception is not a schema verdict: a transient SQLITE_BUSY or I/O error must not
        // disable marker management for the rest of the process, so only column results are cached.
        log(
            `[eidnara] compaction-marker: schema probe failed: ${error instanceof Error ? error.message : String(error)}. ` +
                `Marker injection skipped for this call; the probe retries on the next one.`,
        );
        return false;
    }
}

function getWritableOpenCodeDb(): Database {
    const dbPath = getOpenCodeDbPath();
    if (cachedWriteDb?.path === dbPath) {
        return cachedWriteDb.db;
    }
    if (cachedWriteDb) {
        closeQuietly(cachedWriteDb.db);
        cachedWriteDb = null;
    }
    // Opening a missing path creates an empty database, whose later queries fail with `no such table`.
    if (!existsSync(dbPath)) {
        throw new Error(`OpenCode database not found at ${dbPath} (is OpenCode installed?)`);
    }
    assertSqliteRuntimeGate();
    const db = new Database(dbPath);
    try {
        // Set `busy_timeout` before `journal_mode=WAL` so a cold open waits up to 5 s when OpenCode holds the lock.
        db.exec("PRAGMA busy_timeout=5000");
        db.exec("PRAGMA journal_mode=WAL");
    } catch (error) {
        // A handle that failed initialization is never cached, so it must be closed here or it leaks until GC.
        closeQuietly(db);
        throw error;
    }
    cachedWriteDb = { path: dbPath, db };
    return db;
}

export function closeCompactionMarkerDb(): void {
    if (cachedWriteDb) {
        closeQuietly(cachedWriteDb.db);
        cachedWriteDb = null;
    }
    // Reset the schema-probe cache because the next open can use a different `opencode.db` path.
    cachedSchemaCompatible = null;
}

export interface BoundaryUserMessage {
    id: string;
    timeCreated: number;
}

interface NonSummaryMessageSortKey {
    id: string;
    timeCreated: number;
}

function getNonSummaryMessageSortKey(
    sessionId: string,
    messageId: string,
): NonSummaryMessageSortKey | null {
    const db = getWritableOpenCodeDb();
    const row = db
        .prepare(
            `SELECT time_created, id
             FROM message
             WHERE session_id = ?
               AND id = ?
               AND NOT (COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.summary'), 0) = 1
                        AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.finish'), '') = 'stop')
             LIMIT 1`,
        )
        .get(sessionId, messageId) as { time_created?: unknown; id?: unknown } | undefined;
    if (typeof row?.time_created !== "number" || typeof row.id !== "string") {
        return null;
    }
    return { id: row.id, timeCreated: row.time_created };
}

/**
 * The boundary must be a user message for filterCompacted to work.
 *
 * Exclude compaction summaries (`summary=true`, `finish="stop"`) to keep ordinals consistent with `readRawSessionMessagesFromDb`.
 */
export function findBoundaryUserMessage(
    sessionId: string,
    endMessageId: string,
): BoundaryUserMessage | null {
    const db = getWritableOpenCodeDb();

    // Do not move the boundary when `endMessageId` is absent or identifies an injected summary.
    // A missing target or injected summary makes the pending/direct marker update stale.
    const target = getNonSummaryMessageSortKey(sessionId, endMessageId);
    if (!target) return null;

    // Use `time_created ASC, id ASC` as the canonical order.
    // Filtering `role = 'user'` in SQL prevents long assistant/tool spans from excluding the prior user message.
    const boundary = db
        .prepare(
            `SELECT id, time_created, data
             FROM message
             WHERE session_id = ?
               AND NOT (COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.summary'), 0) = 1
                        AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.finish'), '') = 'stop')
               AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.role'), '') = 'user'
               AND (time_created < ? OR (time_created = ? AND id <= ?))
             ORDER BY time_created DESC, id DESC
             LIMIT 1`,
        )
        .get(sessionId, target.timeCreated, target.timeCreated, target.id) as
        | { id?: unknown; time_created?: unknown; data?: unknown }
        | undefined;

    if (typeof boundary?.id !== "string" || typeof boundary.time_created !== "number") {
        return null;
    }

    return { id: boundary.id, timeCreated: boundary.time_created };
}

export function compareOpenCodeMessagesByCanonicalOrder(
    sessionId: string,
    leftMessageId: string,
    rightMessageId: string,
): number | null {
    const left = getNonSummaryMessageSortKey(sessionId, leftMessageId);
    const right = getNonSummaryMessageSortKey(sessionId, rightMessageId);
    if (!left || !right) return null;
    if (left.timeCreated < right.timeCreated) return -1;
    if (left.timeCreated > right.timeCreated) return 1;
    if (left.id < right.id) return -1;
    if (left.id > right.id) return 1;
    return 0;
}

/**
 *
 * Validate deferred compaction-marker targets before consumption because recompaction, reversion, or partial recompaction can delete them.
 * `getOpenCodeMessageById` propagates OpenCode DB errors.
 *
 * Returns `{ id }` because callers need only existence.
 */
export function getOpenCodeMessageById(
    sessionId: string,
    messageId: string,
): { id: string } | null {
    const db = getWritableOpenCodeDb();
    const row = db
        .prepare(`SELECT id FROM message WHERE session_id = ? AND id = ? LIMIT 1`)
        .get(sessionId, messageId) as { id: string } | null | undefined;
    return row ?? null;
}

export interface CompactionMarkerState {
    sessionId: string;
    boundaryMessageId: string;
    summaryMessageId: string;
    compactionPartId: string;
    summaryPartId: string;
}

export interface InjectCompactionMarkerArgs {
    sessionId: string;
    /** The field stores the raw ordinal of the last compartmentalized message. */
    endOrdinal: number;
    /** The field stores the OpenCode message ID of the last compartmentalized message. */
    endMessageId: string;
    summaryText: string;
    directory: string;
    /** Resolve the boundary before removing the old marker; removing it first can invalidate the cached boundary. */
    resolvedBoundary?: BoundaryUserMessage;
}

/** A surviving `tail_start_id` reference prevents deletion of its stale lineage. */
function removeLegacyMarkerLineageRows(
    db: Database,
    args: {
        sessionId: string;
        boundaryMessageId: string;
        summaryText: string;
        summaryMessageId: string;
        compactionPartId: string;
    },
): void {
    const legacySummaries = db
        .prepare(
            `SELECT m.id
             FROM message m
             WHERE m.session_id = ?
               AND m.id <> ?
               AND COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.summary'), 0) = 1
               AND COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.finish'), '') = 'stop'
               AND COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.parentID'), '') = ?
               AND EXISTS (
                   SELECT 1
                   FROM part p
                   WHERE p.session_id = m.session_id
                     AND p.message_id = m.id
                     AND COALESCE(json_extract(CASE WHEN json_valid(p.data) THEN p.data END, '$.type'), '') = 'text'
                     AND COALESCE(json_extract(CASE WHEN json_valid(p.data) THEN p.data END, '$.text'), '') = ?
               )`,
        )
        .all(
            args.sessionId,
            args.summaryMessageId,
            args.boundaryMessageId,
            args.summaryText,
        ) as Array<{ id?: unknown }>;
    const legacySummaryIds = legacySummaries.flatMap((row) =>
        typeof row.id === "string" ? [row.id] : [],
    );
    if (legacySummaryIds.length === 0) return;

    const tailIndex = loadTailReferenceIndex(db, args.sessionId);
    const staleOwnedPartIds = tailIndex.parts
        .filter(
            (part) =>
                part.messageId === args.boundaryMessageId &&
                part.id !== args.compactionPartId &&
                isEidnaraCompactionPartData(part.data),
        )
        .map((part) => part.id);

    const deleteSummaryParts = db.prepare(
        "DELETE FROM part WHERE session_id = ? AND message_id = ?",
    );
    const deleteSummary = db.prepare("DELETE FROM message WHERE session_id = ? AND id = ?");
    const deletePart = db.prepare("DELETE FROM part WHERE session_id = ? AND id = ?");
    for (const legacySummaryId of legacySummaryIds) {
        if (survivingTailReferences(tailIndex, new Set([legacySummaryId]), new Set())) {
            log(
                `[eidnara] compaction-marker: stale summary RETAINED at boundary ${args.boundaryMessageId} (${legacySummaryId}) — a surviving tail_start_id references it`,
            );
            continue;
        }
        deleteSummaryParts.run(args.sessionId, legacySummaryId);
        deleteSummary.run(args.sessionId, legacySummaryId);
    }
    for (const partId of staleOwnedPartIds) {
        if (survivingTailReferences(tailIndex, new Set([partId]), new Set([partId]))) {
            log(
                `[eidnara] compaction-marker: stale compaction part RETAINED at boundary ${args.boundaryMessageId} (${partId}) — a surviving tail_start_id references it`,
            );
            continue;
        }
        deletePart.run(args.sessionId, partId);
    }
}

/** Upsert one `part` row by deterministic id; retries rewrite the exact canonical row. */
function upsertPartRow(
    db: Database,
    row: {
        id: string;
        messageId: string;
        sessionId: string;
        timeCreated: number;
        timeUpdated: number;
        data: string;
    },
): void {
    db.prepare(
        `INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             message_id = excluded.message_id,
             session_id = excluded.session_id,
             time_created = excluded.time_created,
             time_updated = excluded.time_updated,
             data = excluded.data`,
    ).run(row.id, row.messageId, row.sessionId, row.timeCreated, row.timeUpdated, row.data);
}

/**
 * Returns null when the schema is incompatible or no boundary exists.
 */
export function injectCompactionMarker(
    args: InjectCompactionMarkerArgs,
): CompactionMarkerState | null {
    const db = getWritableOpenCodeDb();
    if (!isOpenCodeSchemaCompatible(db, getOpenCodeDbPath())) {
        return null;
    }

    const boundary =
        args.resolvedBoundary ?? findBoundaryUserMessage(args.sessionId, args.endMessageId);
    if (!boundary) {
        log(
            `[eidnara] compaction-marker: no user message found at or before endMessageId ${args.endMessageId} (ordinal ${args.endOrdinal})`,
        );
        return null;
    }
    // Marker rows use `boundaryTime + 1` in canonical `time_created, id` order.
    // The summary follows the boundary only when the first retained message is created at least 2 ms later.
    // Ids are a pure function of the boundary so retries upsert the same rows.
    const boundaryTime = boundary.timeCreated;
    const markerIdentity = `${args.sessionId}\0${args.endMessageId}`;
    const summaryMsgId = generateMessageId(
        boundaryTime + 1,
        1n,
        `${markerIdentity}\0summary-message`,
    );
    const compactionPartId = generatePartId(boundaryTime, 1n, `${markerIdentity}\0compaction-part`);
    const summaryPartId = generatePartId(boundaryTime + 1, 2n, `${markerIdentity}\0summary-part`);

    const summaryMsgData = JSON.stringify({
        role: "assistant",
        parentID: boundary.id,
        summary: true,
        finish: "stop",
        mode: "compaction",
        agent: "compaction",
        path: { cwd: args.directory, root: args.directory },
        cost: 0,
        tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
        modelID: EIDNARA_PROVIDER_ID,
        providerID: EIDNARA_PROVIDER_ID,
        time: { created: boundaryTime + 1 },
    });

    try {
        // BEGIN IMMEDIATE holds the write lock across the re-checks and the inserts.
        // The write lock prevents a concurrent reversion from deleting either target between re-check and insert.
        // The connection does not enforce foreign keys, so an unchecked insert against a deleted boundary would commit.
        runImmediate(db, () => {
            if (!getOpenCodeMessageById(args.sessionId, boundary.id)) {
                throw new Error(`boundary user message ${boundary.id} no longer exists`);
            }
            // A reverted end target would otherwise commit a summary that reintroduces the reverted content.
            if (!getNonSummaryMessageSortKey(args.sessionId, args.endMessageId)) {
                throw new Error(
                    `end message ${args.endMessageId} no longer exists or is an injected summary`,
                );
            }

            // A committed insert can outlive a failed context-state write, so the canonical-row transaction removes stale lineage.
            removeLegacyMarkerLineageRows(db, {
                sessionId: args.sessionId,
                boundaryMessageId: boundary.id,
                summaryText: args.summaryText,
                summaryMessageId: summaryMsgId,
                compactionPartId,
            });

            // Deterministic IDs make this transaction an upsert on retry. Rewriting
            // the exact canonical row also repairs a partial or stale prior write.
            upsertPartRow(db, {
                id: compactionPartId,
                messageId: boundary.id,
                sessionId: args.sessionId,
                timeCreated: boundaryTime,
                timeUpdated: boundaryTime,
                data: EIDNARA_COMPACTION_PART_DATA,
            });

            db.prepare(
                `INSERT INTO message (id, session_id, time_created, time_updated, data)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                     session_id = excluded.session_id,
                     time_created = excluded.time_created,
                     time_updated = excluded.time_updated,
                     data = excluded.data`,
            ).run(summaryMsgId, args.sessionId, boundaryTime + 1, boundaryTime + 1, summaryMsgData);

            upsertPartRow(db, {
                id: summaryPartId,
                messageId: summaryMsgId,
                sessionId: args.sessionId,
                timeCreated: boundaryTime + 1,
                timeUpdated: boundaryTime + 1,
                data: JSON.stringify({ type: "text", text: args.summaryText }),
            });
        });

        log(
            `[eidnara] compaction-marker: injected boundary at user msg ${boundary.id} (ordinal ~${args.endOrdinal}), summary msg ${summaryMsgId}`,
        );

        return {
            sessionId: args.sessionId,
            boundaryMessageId: boundary.id,
            summaryMessageId: summaryMsgId,
            compactionPartId,
            summaryPartId,
        };
    } catch (error) {
        log(
            `[eidnara] compaction-marker: injection failed: ${error instanceof Error ? error.message : String(error)}`,
        );
        return null;
    }
}

// ── Foreign-marker scan (fork-orphan hygiene) ─────────────

/**
 *
 * `summaryMessageIds` contains completed assistant summaries with `summary=true` and `finish="stop"`.
 * Each `summaryMessageIds` entry is parented to the boundary user message.
 * `summaryMessageIds` includes only summaries with Eidnara's provider identity and is never empty.
 * OpenCode-native `/compact` summaries use their real provider ID and are excluded.
 * Excluding summaries without Eidnara's provider ID prevents callers from deleting native compactions.
 */
export interface SessionCompactionMarkerRows {
    /** `compactionPartId` identifies the `type:"compaction"` part attached to the boundary user message. */
    compactionPartId: string;
    boundaryMessageId: string;
    /** `summaryMessageIds` identifies eidnara summaries parented to the boundary user message. */
    summaryMessageIds: string[];
}

interface EidnaraSummaryRow {
    id: string;
    /** `parentId` is empty when the summary's boundary reference is missing. */
    parentId: string;
}

function selectEidnaraSummaryRows(db: Database, sessionId: string): EidnaraSummaryRow[] {
    const rows = db
        .prepare(
            `SELECT id, COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.parentID'), '') AS parent_id
             FROM message
             WHERE session_id = ?
               AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.summary'), 0) = 1
               AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.finish'), '') = 'stop'
               AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.providerID'), '') = ?`,
        )
        .all(sessionId, EIDNARA_PROVIDER_ID) as Array<{ id?: unknown; parent_id?: unknown }>;
    return rows.flatMap((row) =>
        typeof row.id === "string"
            ? [{ id: row.id, parentId: typeof row.parent_id === "string" ? row.parent_id : "" }]
            : [],
    );
}

interface SessionCompactionPart {
    id: string;
    messageId: string;
    data: unknown;
    tailStartId: string | null;
}

/** Native and plugin-owned compaction parts are both returned; callers apply the ownership test. */
function selectSessionCompactionParts(db: Database, sessionId: string): SessionCompactionPart[] {
    const rows = db
        .prepare(
            `SELECT id, message_id, data
             FROM part
             WHERE session_id = ?
               AND COALESCE(json_extract(CASE WHEN json_valid(data) THEN data END, '$.type'), '') = 'compaction'`,
        )
        .all(sessionId) as Array<{ id?: unknown; message_id?: unknown; data?: unknown }>;
    const parts: SessionCompactionPart[] = [];
    for (const row of rows) {
        if (typeof row.id !== "string" || typeof row.message_id !== "string") continue;
        let data: unknown;
        try {
            data = typeof row.data === "string" ? JSON.parse(row.data) : row.data;
        } catch {
            data = null;
        }
        parts.push({
            id: row.id,
            messageId: row.message_id,
            data,
            tailStartId: dataReferencesTailStart(data),
        });
    }
    return parts;
}

interface TailReferenceIndex {
    parts: SessionCompactionPart[];
    messageTailStartIds: Set<string>;
}

function loadTailReferenceIndex(db: Database, sessionId: string): TailReferenceIndex {
    const parts = selectSessionCompactionParts(db, sessionId);
    const messageTailRefs = db
        .prepare(
            `SELECT id, data
             FROM message
             WHERE session_id = ?
               AND json_extract(CASE WHEN json_valid(data) THEN data END, '$.compaction.tail_start_id') IS NOT NULL`,
        )
        .all(sessionId) as Array<{ id?: unknown; data?: unknown }>;
    const messageTailStartIds = new Set<string>();
    for (const row of messageTailRefs) {
        if (typeof row.id !== "string") continue;
        let data: unknown;
        try {
            data = typeof row.data === "string" ? JSON.parse(row.data) : row.data;
        } catch {
            data = null;
        }
        const ref = dataReferencesTailStart(data);
        if (ref) messageTailStartIds.add(ref);
    }
    return { parts, messageTailStartIds };
}

/** Parts in `excludedPartIds` are the ones the deletion itself removes; their references vanish with them. */
function survivingTailReferences(
    index: TailReferenceIndex,
    rowsToDelete: ReadonlySet<string>,
    excludedPartIds: ReadonlySet<string>,
): boolean {
    if (
        index.parts.some(
            (part) =>
                !excludedPartIds.has(part.id) &&
                part.tailStartId !== null &&
                rowsToDelete.has(part.tailStartId),
        )
    ) {
        return true;
    }
    for (const id of rowsToDelete) {
        if (index.messageTailStartIds.has(id)) return true;
    }
    return false;
}

/**
 *
 * Used by the fork-orphan hygiene pass: OpenCode's `/fork` copies the
 * parent session's message rows — including this plugin's compaction marker
 * rows — into the fork. Forks do not record copied marker rows in the daemon store.
 * Callers diff the listed markers against that store and repair the ones it does not own.
 *
 * The hygiene pass retries scan failures later instead of treating them as fatal transform errors.
 */
export function listSessionCompactionMarkers(sessionId: string): SessionCompactionMarkerRows[] {
    const db = getWritableOpenCodeDb();

    const summariesByBoundary = new Map<string, string[]>();
    for (const row of selectEidnaraSummaryRows(db, sessionId)) {
        if (row.parentId.length === 0) continue;
        const ids = summariesByBoundary.get(row.parentId) ?? [];
        ids.push(row.id);
        summariesByBoundary.set(row.parentId, ids);
    }

    const markers: SessionCompactionMarkerRows[] = [];
    for (const part of selectSessionCompactionParts(db, sessionId)) {
        const summaryMessageIds = summariesByBoundary.get(part.messageId);
        if (!summaryMessageIds || !isEidnaraCompactionPartData(part.data)) continue;
        markers.push({
            compactionPartId: part.id,
            boundaryMessageId: part.messageId,
            summaryMessageIds: [...summaryMessageIds],
        });
    }
    return markers;
}

/**
 * `"retained"` means a surviving `tail_start_id` references a row the removal would delete; the
 * marker is left in place and a retry will not change the outcome until that reference is gone.
 * `"failed"` means opening or executing the transaction threw (including SQLITE_BUSY) and the
 * removal can be retried.
 */
export type MarkerRemovalOutcome = "removed" | "retained" | "failed";

/**
 *
 * `filterCompacted` requires a compaction part to break, so deleting that part stops it from ignoring the marker.
 * Deleting summary rows prevents stale `[Compacted by eidnara]` messages from remaining in fork history.
 *
 * `protectedSummaryMessageId` identifies the caller-owned summary message that cleanup must retain.
 *
 * The tail-reference preflight and the deletes run under one `BEGIN IMMEDIATE` transaction.
 */
export function removeForeignCompactionMarker(
    sessionId: string,
    marker: SessionCompactionMarkerRows,
    protectedSummaryMessageId: string | null,
): MarkerRemovalOutcome {
    try {
        const db = getWritableOpenCodeDb();
        return runImmediate(db, (): MarkerRemovalOutcome => {
            const summaryIds = marker.summaryMessageIds.filter(
                (id) => id !== protectedSummaryMessageId,
            );
            const rowsToDelete = new Set<string>([...summaryIds, marker.compactionPartId]);
            const index = loadTailReferenceIndex(db, sessionId);
            if (survivingTailReferences(index, rowsToDelete, new Set([marker.compactionPartId]))) {
                log(
                    `[eidnara] compaction-marker: foreign-marker removal RETAINED (${sessionId}, part ${marker.compactionPartId}) — a surviving tail_start_id references a row the removal would delete`,
                );
                return "retained";
            }

            const deletePartsOfMessage = db.prepare(
                "DELETE FROM part WHERE session_id = ? AND message_id = ?",
            );
            const deleteMessage = db.prepare("DELETE FROM message WHERE session_id = ? AND id = ?");
            for (const summaryMessageId of summaryIds) {
                deletePartsOfMessage.run(sessionId, summaryMessageId);
                deleteMessage.run(sessionId, summaryMessageId);
            }
            db.prepare("DELETE FROM part WHERE session_id = ? AND id = ?").run(
                sessionId,
                marker.compactionPartId,
            );
            return "removed";
        });
    } catch (error) {
        log(
            `[eidnara] compaction-marker: foreign-marker removal failed (${sessionId}, part ${marker.compactionPartId}): ${error instanceof Error ? error.message : String(error)}`,
        );
        return "failed";
    }
}

/**
 * Result of the compaction-off flip cleanup over one session's opencode.db
 * rows. Counts are row-level so the transition can both gate
 * the flip notice ("cleared something") and prove idempotence (a second run
 * reports zero removed rows).
 */
export interface EidnaraOwnedMarkerCleanupResult {
    /**
     * The result is true only when cleanup skips no eidnara-owned lineage.
     * A false `verified` leaves the mode transition retryable.
     * A false `verified` prevents recording a successful flip while a marker can still hide history.
     */
    verified: boolean;
    /** `removedLineages` counts plugin-owned marker lineages whose compaction part and summary rows were all removed. */
    removedLineages: number;
    removedRows: number;
    /**
     * `retainedLineages` counts lineages retained when a surviving `tail_start_id` references a row deletion would remove.
     * A missing `tail_start_id` target makes OpenCode's `tailIndex` resolve to `-1` and bypass reordering.
     * The cleanup retains lineages it cannot retarget.
     * retains.
     */
    retainedLineages: number;
}

function dataReferencesTailStart(data: unknown): string | null {
    if (typeof data !== "object" || data === null) return null;
    const record = data as Record<string, unknown>;
    if (typeof record.tail_start_id === "string" && record.tail_start_id.length > 0) {
        return record.tail_start_id;
    }
    const nested = record.compaction;
    if (typeof nested === "object" && nested !== null) {
        const nestedTail = (nested as Record<string, unknown>).tail_start_id;
        if (typeof nestedTail === "string" && nestedTail.length > 0) return nestedTail;
    }
    return null;
}

/**
 * Delete every Eidnara-owned compaction-marker lineage for a session
 * from opencode.db. This is the flip-off transition's primary mechanism:
 * with the plugin no longer injecting `<session-history>`,
 * a surviving plugin marker would keep `filterCompacted` hiding pre-boundary
 * history with nothing to replace it — orphaned context. Deleting the plugin
 * pairs lets OpenCode recompute filtering live from the surviving rows, as if
 * the plugin markers never existed (peer-verified newest-completed-summary
 * semantics; older markers inside the retained tail do not define the
 * boundary). Native compaction rows are never matched: ownership keys on
 * Eidnara-specific signatures (the `eidnara` provider identity on summary
 * messages, the exact plugin marker summary text for legacy lineages, and the
 * `eidnara` key on the compaction part payload) plus session identity.
 *
 * The transaction atomically removes each compaction part with its summary lineage.
 * Deleting only the compaction part would leave its summary message in model history.
 * The boundary user message is real user history; cleanup deletes only its plugin-injected compaction part.
 * Preflight retains a lineage when a surviving compaction marker references a row that deletion would remove through `tail_start_id`.
 * The preflight scans and every deletion run under one `BEGIN IMMEDIATE` transaction, so a
 * `tail_start_id` that OpenCode writes concurrently cannot land between the scan and the delete
 * it would have retained.
 *
 * Idempotent: absent rows delete as a no-op (second run reports zeros).
 * Errors propagate.
 */
export function removeEidnaraOwnedCompactionMarkers(
    sessionId: string,
    summaryText: string,
): EidnaraOwnedMarkerCleanupResult {
    const db = getWritableOpenCodeDb();
    if (!isOpenCodeSchemaCompatible(db, getOpenCodeDbPath())) {
        // Schema incompatibility leaves all rows in place because cleanup cannot verify its DELETE predicates.
        return {
            verified: false,
            removedLineages: 0,
            removedRows: 0,
            retainedLineages: 0,
        };
    }

    return runImmediate(db, () =>
        removeEidnaraOwnedCompactionMarkersLocked(db, sessionId, summaryText),
    );
}

function removeEidnaraOwnedCompactionMarkersLocked(
    db: Database,
    sessionId: string,
    summaryText: string,
): EidnaraOwnedMarkerCleanupResult {
    const canonicalSummaries = selectEidnaraSummaryRows(db, sessionId);
    const legacySummaries = db
        .prepare(
            `SELECT m.id, COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.parentID'), '') AS parent_id
             FROM message m
             WHERE m.session_id = ?
               AND COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.summary'), 0) = 1
               AND COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.finish'), '') = 'stop'
               AND COALESCE(json_extract(CASE WHEN json_valid(m.data) THEN m.data END, '$.providerID'), '') <> ?
               AND EXISTS (
                   SELECT 1
                   FROM part p
                   WHERE p.session_id = m.session_id
                     AND p.message_id = m.id
                     AND COALESCE(json_extract(CASE WHEN json_valid(p.data) THEN p.data END, '$.type'), '') = 'text'
                     AND COALESCE(json_extract(CASE WHEN json_valid(p.data) THEN p.data END, '$.text'), '') = ?
               )`,
        )
        .all(sessionId, EIDNARA_PROVIDER_ID, summaryText) as Array<{
        id?: unknown;
        parent_id?: unknown;
    }>;

    const summariesByBoundary = new Map<string, Set<string>>();
    const orphanSummaryIds = new Set<string>();
    const summaryRows: EidnaraSummaryRow[] = [
        ...canonicalSummaries,
        ...legacySummaries.flatMap((row) =>
            typeof row.id === "string"
                ? [{ id: row.id, parentId: typeof row.parent_id === "string" ? row.parent_id : "" }]
                : [],
        ),
    ];
    for (const row of summaryRows) {
        if (row.parentId.length > 0) {
            const set = summariesByBoundary.get(row.parentId) ?? new Set<string>();
            set.add(row.id);
            summariesByBoundary.set(row.parentId, set);
        } else {
            // A stranded plugin summary whose boundary is gone is still plugin-owned
            // Cleanup removes stranded plugin summaries because they remain visible in model history.
            orphanSummaryIds.add(row.id);
        }
    }

    // Preflight parses every session compaction part once so it sees surviving native parts and evaluates each boundary against all of its parts.
    // Preflight includes message-level V2 compaction `tail_start_id` references; any reference to a deletion target retains the lineage.
    const tailIndex = loadTailReferenceIndex(db, sessionId);

    let removedLineages = 0;
    let removedRows = 0;
    let retainedLineages = 0;

    const deletePartsOfMessage = db.prepare(
        "DELETE FROM part WHERE session_id = ? AND message_id = ?",
    );
    const deleteMessage = db.prepare("DELETE FROM message WHERE session_id = ? AND id = ?");
    const deletePart = db.prepare("DELETE FROM part WHERE session_id = ? AND id = ?");

    const deleteSummaries = (summaryIds: Set<string>): number => {
        let rows = 0;
        for (const summaryId of summaryIds) {
            rows += deletePartsOfMessage.run(sessionId, summaryId).changes;
            rows += deleteMessage.run(sessionId, summaryId).changes;
        }
        return rows;
    };

    for (const [boundaryMessageId, summaryIds] of summariesByBoundary) {
        // A native compaction part on the same boundary lacks the payload key and is retained.
        const ownedPartIds = new Set(
            tailIndex.parts
                .filter(
                    (part) =>
                        part.messageId === boundaryMessageId &&
                        isEidnaraCompactionPartData(part.data),
                )
                .map((part) => part.id),
        );

        // The preflight includes every row this lineage would delete.
        const rowsToDelete = new Set<string>([...summaryIds, ...ownedPartIds]);
        if (survivingTailReferences(tailIndex, rowsToDelete, ownedPartIds)) {
            retainedLineages += 1;
            log(
                `[eidnara] compaction-marker: flip-off cleanup RETAINED lineage at boundary ${boundaryMessageId} — a surviving tail_start_id references a row the deletion would remove`,
            );
            continue;
        }

        let rows = deleteSummaries(summaryIds);
        for (const partId of ownedPartIds) {
            rows += deletePart.run(sessionId, partId).changes;
        }
        if (rows > 0 || summaryIds.size > 0 || ownedPartIds.size > 0) {
            removedLineages += 1;
            removedRows += rows;
        }
    }

    if (orphanSummaryIds.size > 0) {
        // Orphan summaries have no boundary to preflight.
        if (survivingTailReferences(tailIndex, orphanSummaryIds, new Set())) {
            retainedLineages += 1;
        } else {
            const rows = deleteSummaries(orphanSummaryIds);
            removedLineages += 1;
            removedRows += rows;
        }
    }

    if (removedLineages > 0 || retainedLineages > 0) {
        log(
            `[eidnara] compaction-marker: flip-off cleanup for ${sessionId} removed ${removedLineages} lineage(s) (${removedRows} rows), retained ${retainedLineages}`,
        );
    }
    return {
        verified: retainedLineages === 0,
        removedLineages,
        removedRows,
        retainedLineages,
    };
}

/** The tail-reference preflight and deletes run in one `BEGIN IMMEDIATE` transaction. */
export function removeCompactionMarker(state: CompactionMarkerState): MarkerRemovalOutcome {
    try {
        const db = getWritableOpenCodeDb();
        return runImmediate(db, (): MarkerRemovalOutcome => {
            const rowsToDelete = new Set([
                state.summaryPartId,
                state.summaryMessageId,
                state.compactionPartId,
            ]);
            const index = loadTailReferenceIndex(db, state.sessionId);
            if (survivingTailReferences(index, rowsToDelete, new Set([state.compactionPartId]))) {
                log(
                    `[eidnara] compaction-marker: removal RETAINED (${state.sessionId}, part ${state.compactionPartId}) — a surviving tail_start_id references a row the removal would delete`,
                );
                return "retained";
            }
            const deletePart = db.prepare("DELETE FROM part WHERE session_id = ? AND id = ?");
            deletePart.run(state.sessionId, state.summaryPartId);
            db.prepare("DELETE FROM message WHERE session_id = ? AND id = ?").run(
                state.sessionId,
                state.summaryMessageId,
            );
            deletePart.run(state.sessionId, state.compactionPartId);
            return "removed";
        });
    } catch (error) {
        log(
            `[eidnara] compaction-marker: removal failed: ${error instanceof Error ? error.message : String(error)}`,
        );
        return "failed";
    }
}
