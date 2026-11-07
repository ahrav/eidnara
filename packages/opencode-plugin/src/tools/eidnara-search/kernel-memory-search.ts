/**
 * Memory search ranks kernel memory rows from the project's visible rows,
 * which the daemon serves in bounded pages.
 */

import { createHash } from "node:crypto";

import type { Deadline } from "../../shared/host-client";
import {
    isAvailable,
    isMemoryDecisionRow,
    type KernelClient,
    MAX_READ_OBJECT_IDS,
    type MemoryState,
    type ReadCursor,
    type ReadRow,
    type RouteCausality,
    type Surface,
    unavailable,
} from "../../shared/kernel-client";
import {
    ANTI_MEMORY_CATEGORY,
    type AntiMemoryPayload,
    antiMemoryExpired,
    parseAntiMemoryContent,
} from "../../shared/kernel-client/anti-memory";
import { splitQueryOperands } from "./bounds";

export interface MemorySearchResult {
    source: "memory";
    content: string;
    score: number;
    /** Kernel object id of the memory (`mem_<32hex>`). */
    objectId: string;
    category: string;
    /** `exact` for an object-id lookup, `lexical` for term-overlap ranking, `fused` for the daemon's ranked route. */
    matchType: KernelMemoryMatchType;
    /** The route lanes that ranked a `fused` result. */
    lanes?: readonly string[];
    sourceName?: string;
    policyLabel?: string;
    /** The causal class the daemon's route validated a `fused` result's claim under: `direct_observation`, `derived_reinjection`, or `unknown`. */
    provenance?: RouteCausality;
    /** Exact SHA-256 digest of the served revision's content bytes. */
    contentDigest?: string;
}

export interface AntiMemorySearchResult {
    source: "anti_memory";
    score: number;
    /** Kernel object id of the anti-memory (`mem_<32hex>`). */
    objectId: string;
    contentDigest: string;
    normalizedHash: string;
    trigger: string;
    rejectedStrategy: string;
    rejectionReason: string;
    saferAlternative: string | null;
    /** Decision rationale served alongside the anti-memory summary; lexical ranking scores it, so renderers that show ranked text include it. */
    rationale?: string;
    matchType: KernelMemoryMatchType | "semantic";
    /** The route lanes that ranked a `fused` result. */
    lanes?: readonly string[];
    policyLabel?: string;
    /** The causal class the daemon's route validated a `fused` result's claim under. */
    provenance?: RouteCausality;
}

export type KernelMemorySearchResult = MemorySearchResult | AntiMemorySearchResult;

/** How a result matched: `exact` for an object-id lookup, `lexical` for term-overlap ranking, `fused` for the daemon's ranked route. */
export type KernelMemoryMatchType = "exact" | "lexical" | "fused";

export type ChunkedObjectRead =
    | {
          ok: true;
          rows: ReadRow[];
          /** The commit sequence all rows were read at. */
          knownAsOf: number;
          unresolvedObjectIds: string[];
      }
    | { ok: false; state: MemoryState };

/**
 * Reads the named objects through the daemon's id filter, halving any chunk
 * whose reply is truncated: the filter bypasses the daemon's row cap but not
 * its serialization byte budget, and the budget keeps a newest-first prefix,
 * so a truncated filtered read has silently dropped the oldest named rows.
 * The daemon's budget holds at least eight worst-case rows (each decision
 * text field is redaction-capped well under an eighth of the budget), so
 * halving reaches complete chunks before the single-id floor; a single id
 * whose read still reports truncated without serving its row lands in
 * `unresolvedObjectIds` instead of passing as proven-missing.
 *
 * Later chunks use the first reply's `known_as_of` as `asOf` to pin all rows to one store state. A mismatched `known_as_of` means chunks do not share one store state, and the read fails closed with `snapshot_diverged`.
 */
export async function readObjectRowsChunked(args: {
    client: KernelClient;
    surface: Surface;
    gated: boolean;
    /** Any length; the read issues one filtered request per `MAX_READ_OBJECT_IDS` ids, the client's filter bound. */
    objectIds: readonly string[];
    signal?: AbortSignal;
    /** Each chunk's read receives this deadline's remainder, so every chunk ends by the same instant. */
    deadline?: Deadline;
}): Promise<ChunkedObjectRead> {
    const rows: ReadRow[] = [];
    const unresolvedObjectIds: string[] = [];
    let snapshot: number | null = null;
    const pending: (readonly string[])[] = [];
    for (let start = 0; start < args.objectIds.length; start += MAX_READ_OBJECT_IDS) {
        pending.push(args.objectIds.slice(start, start + MAX_READ_OBJECT_IDS));
    }
    while (pending.length > 0) {
        const chunk = pending.pop() as readonly string[];
        const read = await args.client.read({
            surface: args.surface,
            gated: args.gated,
            objectIds: chunk,
            asOf: snapshot,
            ...(args.signal ? { signal: args.signal } : {}),
            ...(args.deadline ? { deadlineMs: args.deadline.remainingMs() } : {}),
        });
        if (!isAvailable(read)) return { ok: false, state: read.state };
        if (snapshot === null) {
            snapshot = read.known_as_of;
        } else if (read.known_as_of !== snapshot) {
            return { ok: false, state: unavailable("snapshot_diverged") };
        }
        if (read.truncated && chunk.length > 1) {
            const mid = Math.ceil(chunk.length / 2);
            pending.push(chunk.slice(0, mid), chunk.slice(mid));
            continue;
        }
        rows.push(...read.rows);
        if (
            read.truncated &&
            chunk.length === 1 &&
            !read.rows.some((row) => row.object.object_id === chunk[0])
        ) {
            unresolvedObjectIds.push(chunk[0] as string);
        }
    }
    return { ok: true, rows, knownAsOf: snapshot ?? 0, unresolvedObjectIds };
}

const OBJECT_ID = /^mem_[0-9a-f]{32}$/;
const OBJECT_ID_AT_COMMIT = /^(mem_[0-9a-f]{32})@\d+$/;

/** The bare object id of an id or `id@commit` token; `null` for ordinary text. */
function objectIdFromToken(token: string): string | null {
    if (OBJECT_ID.test(token)) return token;
    return OBJECT_ID_AT_COMMIT.exec(token)?.[1] ?? null;
}

/** Object ids when the whole query is a list of ids or `id@commit` tokens; `null` for ordinary text. Results render bare ids; the `@commit` suffix is accepted because earlier releases rendered it, and it is ignored for the lookup, since the store serves one live row per id. */
export function parseObjectIdQuery(query: string): string[] | null {
    const tokens = query
        .trim()
        .split(/[\s,]+/)
        .filter(Boolean);
    if (tokens.length === 0) return null;
    const ids: string[] = [];
    for (const token of tokens) {
        const id = objectIdFromToken(token);
        if (id === null) return null;
        ids.push(id);
    }
    return [...new Set(ids)];
}

function queryTerms(query: string): string[] {
    return [...new Set(splitQueryOperands(query.toLowerCase()).filter((term) => term.length >= 2))];
}

function rowText(row: ReadRow): string {
    const decision = row.decision;
    return decision ? `${decision.payload.summary}\n${decision.payload.rationale}` : "";
}

/** The parsed payload of a rejected-approach row: `null` when its summary is not the field-labeled anti-memory text form, and `undefined` for every other row. */
export function antiMemoryPayloadOfRow(row: ReadRow): AntiMemoryPayload | null | undefined {
    const decision = row.decision;
    if (decision?.decision_kind !== ANTI_MEMORY_CATEGORY) return undefined;
    try {
        return parseAntiMemoryContent(decision.payload.summary);
    } catch {
        return null;
    }
}

/** A rejected-approach row emits `anti_memory`. An unparseable stored summary becomes a warning carrying the raw summary as the rejected strategy. Callers pass a previously parsed `antiMemory` value to reuse the parse result. */
export function memoryResultFromRow(
    row: ReadRow,
    score: number,
    matchType: KernelMemoryMatchType,
    antiMemory = antiMemoryPayloadOfRow(row),
): KernelMemorySearchResult {
    const decision = row.decision;
    if (decision?.decision_kind === ANTI_MEMORY_CATEGORY) {
        const payload = antiMemory ?? null;
        // The digest of the served summary stands in for both content hashes.
        const digest = createHash("sha256").update(decision.payload.summary, "utf8").digest("hex");
        return {
            source: "anti_memory",
            score,
            objectId: row.object.object_id,
            contentDigest: digest,
            normalizedHash: digest,
            trigger: payload?.trigger ?? "",
            rejectedStrategy: payload?.rejectedStrategy ?? decision.payload.summary,
            rejectionReason:
                payload?.rejectionReason ??
                "stored anti-memory payload is unparseable; the approach stays rejected",
            saferAlternative: payload?.saferAlternative ?? null,
            // `rowText` ranks the rationale, so the warning carries it too; a hit matched only in the rationale would otherwise display none of the matched terms and read as unrelated.
            ...(decision.payload.rationale ? { rationale: decision.payload.rationale } : {}),
            matchType,
            ...(row.labeled ? { policyLabel: "labeled" } : {}),
        };
    }
    return {
        source: "memory",
        // `rowText` ranks the rationale, so the rationale renders too; a hit matched only in the rationale would otherwise display none of the matched terms and read as unrelated.
        content: decision?.payload.rationale
            ? `${decision.payload.summary}\n${decision.payload.rationale}`
            : (decision?.payload.summary ?? ""),
        score,
        objectId: row.object.object_id,
        category: decision?.decision_kind ?? row.object.object_kind,
        matchType,
        ...(row.labeled ? { policyLabel: "labeled" } : {}),
    };
}

export interface KernelMemorySearchArgs {
    rows: readonly ReadRow[];
    query: string;
    limit: number;
    /** Objects already rendered in the injected baseline; a hit on one spends budget on visible text. */
    excludeObjectIds?: ReadonlySet<string> | null;
    /** Expiry filtering compares timestamps against `nowMs`; defaults to the wall clock. */
    nowMs?: number;
}

/** An anti-memory past its rendered expiry never surfaces as a search hit; an unparseable summary never counts as expired. */
function isExpiredAntiMemoryRow(row: ReadRow, nowMs: number): boolean {
    const payload = antiMemoryPayloadOfRow(row);
    return payload != null && antiMemoryExpired(payload, nowMs);
}

/** An object-id query resolves exactly (in id order); text ranks rows by the share of query terms their summary and rationale contain, ties broken by newest first. `excludeObjectIds` applies only to lexical searches; object-id queries ignore `excludeObjectIds` because an explicit id names one object and resolves even when the injected baseline already renders it. Either path returns `null` when nothing matches so the caller can fall through to the other sources. */
export function searchKernelMemoryRows(
    args: KernelMemorySearchArgs,
): KernelMemorySearchResult[] | null {
    const nowMs = args.nowMs ?? Date.now();
    const decisionRows = args.rows.filter((row) => isSearchableMemoryRow(row, nowMs));
    const ids = parseObjectIdQuery(args.query);
    if (ids) {
        const byId = new Map(decisionRows.map((row) => [row.object.object_id, row]));
        const hits = ids
            .map((id) => byId.get(id))
            .filter((row): row is ReadRow => row !== undefined)
            .slice(0, args.limit)
            .map((row) => memoryResultFromRow(row, 1, "exact"));
        return hits.length > 0 ? hits : null;
    }
    const candidates = decisionRows.filter(
        (row) => !args.excludeObjectIds?.has(row.object.object_id),
    );
    const scored = rankLexicalRows(candidates, queryTerms(args.query), args.limit).map((entry) =>
        memoryResultFromRow(entry.row, entry.score, "lexical"),
    );
    return scored.length > 0 ? scored : null;
}

/** Each row's rank is independent of other rows, so merging each partition's top `limit` rows preserves the union's top `limit` rows. */
function rankLexicalRows(
    rows: readonly ReadRow[],
    terms: readonly string[],
    limit: number,
): { row: ReadRow; score: number }[] {
    if (terms.length === 0) return [];
    return rows
        .map((row) => {
            const text = rowText(row).toLowerCase();
            const matched = terms.filter((term) => text.includes(term)).length;
            return { row, score: matched / terms.length };
        })
        .filter((entry) => entry.score > 0)
        .sort(
            (left, right) =>
                right.score - left.score ||
                right.row.object.created_commit_seq - left.row.object.created_commit_seq ||
                left.row.object.object_id.localeCompare(right.row.object.object_id),
        )
        .slice(0, limit);
}

function followsCursor(cursor: ReadCursor, row: ReadRow): boolean {
    const seq = row.object.created_commit_seq;
    return (
        seq < cursor.createdCommitSeq ||
        (seq === cursor.createdCommitSeq && row.object.object_id > cursor.objectId)
    );
}

/**
 * The default scan reads up to sixteen pages at the first page's snapshot. A page holds at most
 * 8,192 rows and 8 MiB of row bytes, so sixteen pages cover 131,072 memories (a project that keeps
 * 100 memories a day for more than three years) and transfer at most 128 MiB.
 */
export const MAX_SNAPSHOT_SCAN_PAGES = 16;

/** One scan spends at most the deadline one memory read had before the scan paged, so the fallback answers no later than it did; pages the deadline leaves unread are reported as truncation. */
export const SNAPSHOT_SCAN_DEADLINE_MS = 10_000;

/** The rows a lexical search ranks: memory decisions, minus anti-memories past their expiry. */
function isSearchableMemoryRow(row: ReadRow, nowMs: number): boolean {
    return isMemoryDecisionRow(row) && !isExpiredAntiMemoryRow(row, nowMs);
}

export type SnapshotScan =
    | { ok: true; rows: ReadRow[]; truncated: boolean }
    | { ok: false; state: MemoryState };

/**
 * Reads the surface page by page at the first page's snapshot. The retained rows preserve the
 * top `limit` lexical results among eligible rows from the pages read. Only the first page is
 * gated; later pages read the snapshot that gate admitted. `truncated` reports rows left unread,
 * after `maxPages` pages, past `deadline`, or when the daemon refuses a later page or answers it from
 * another snapshot. A cancelled call answers its state, as a refused first page does.
 */
export async function scanSnapshotForLexicalSearch(args: {
    client: KernelClient;
    surface: Surface;
    gated: boolean;
    query: string;
    limit: number;
    nowMs: number;
    signal?: AbortSignal;
    /** Each page's read receives this deadline's remainder. */
    deadline?: Deadline;
    maxPages?: number;
}): Promise<SnapshotScan> {
    const terms = queryTerms(args.query);
    const maxPages = args.maxPages ?? MAX_SNAPSHOT_SCAN_PAGES;
    let kept: ReadRow[] = [];
    let asOf: number | null = null;
    let before: ReadCursor | undefined;
    for (let page = 0; page < maxPages; page += 1) {
        const read = await args.client.read({
            surface: args.surface,
            gated: page === 0 && args.gated,
            ...(asOf === null ? {} : { asOf }),
            ...(before === undefined ? {} : { before }),
            ...(args.signal ? { signal: args.signal } : {}),
            ...(args.deadline ? { deadlineMs: args.deadline.remainingMs() } : {}),
        });
        if (!isAvailable(read)) {
            const cancelled = read.state.kind === "cancelled" || args.signal?.aborted === true;
            return asOf === null || cancelled
                ? { ok: false, state: read.state }
                : { ok: true, rows: kept, truncated: true };
        }
        const cursor = before;
        const continuesSnapshot =
            asOf === null ||
            (read.known_as_of === asOf &&
                (cursor === undefined || read.rows.every((row) => followsCursor(cursor, row))));
        if (!continuesSnapshot) return { ok: true, rows: kept, truncated: true };
        asOf = read.known_as_of;
        const candidates = read.rows.filter((row) => isSearchableMemoryRow(row, args.nowMs));
        kept = rankLexicalRows([...kept, ...candidates], terms, args.limit).map(
            (entry) => entry.row,
        );
        const last = read.rows.at(-1);
        if (!read.truncated || last === undefined || terms.length === 0) {
            return { ok: true, rows: kept, truncated: read.truncated };
        }
        before = {
            createdCommitSeq: last.object.created_commit_seq,
            objectId: last.object.object_id,
        };
    }
    return { ok: true, rows: kept, truncated: true };
}
