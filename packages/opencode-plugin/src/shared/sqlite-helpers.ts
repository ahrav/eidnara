/**
 * These helpers isolate bun:sqlite and node:sqlite API differences from call sites.
 */

import type { Database } from "./sqlite";

/**
 *
 * bun:sqlite suppresses close errors when `throwOnError` is `false`.
 * node:sqlite throws when `db.close()` receives an already-closed handle.
 * closeQuietly suppresses `db.close()` errors to match bun:sqlite's `throwOnError = false` behavior.
 */
export function closeQuietly(db: Database | null | undefined): void {
    if (!db) return;
    try {
        db.close();
    } catch {}
}

/**
 * Builds a `json_extract` that yields NULL for a malformed `column` instead of raising `malformed JSON`.
 * `CASE` evaluates only the taken branch, so the extract never runs on an invalid document; an `AND`
 * guard has no such ordering guarantee. `column` and `path` are code literals, never caller input. commentlint: allow(JUDGE)
 */
export function jsonField(column: string, path: string): string {
    return `CASE WHEN json_valid(${column}) = 1 THEN json_extract(${column}, '${path}') END`;
}
