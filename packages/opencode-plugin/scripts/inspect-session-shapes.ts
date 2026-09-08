#!/usr/bin/env bun
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import { resolveOpenCodeDatabasePath } from "../src/shared/opencode-database-path";
import { Database } from "../src/shared/sqlite";

const piSessionsDir = join(homedir(), ".pi", "agent", "sessions");

const SHAPE_MAX_DEPTH = 6;
const SESSIONS_PER_CRITERION = 5;
const SAMPLE_SCAN_LIMIT_PER_SESSION = 10000;

/** Session entries hold user prompts and tool output, so samples print as type skeletons rather than values. */
function shapeOf(value: unknown, depth = 0): unknown {
    if (value === null) return "null";
    if (Array.isArray(value)) {
        if (depth >= SHAPE_MAX_DEPTH) return "[...]";
        return value.length === 0 ? [] : [shapeOf(value[0], depth + 1)];
    }
    switch (typeof value) {
        case "string":
            return `string(${value.length})`;
        case "object": {
            if (depth >= SHAPE_MAX_DEPTH) return "{...}";
            const out: Record<string, unknown> = {};
            for (const [key, inner] of Object.entries(value as Record<string, unknown>)) {
                out[key] = shapeOf(inner, depth + 1);
            }
            return out;
        }
        default:
            return typeof value;
    }
}

/** `JSON.parse` accepts `null` and scalars, which have no `type`; those count as malformed alongside parse errors. */
function parseEntry(raw: string): { type?: string } | undefined {
    let parsed: unknown;
    try {
        parsed = JSON.parse(raw);
    } catch {
        return undefined;
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return undefined;
    return parsed as { type?: string };
}

function printSamples(samples: Map<string, unknown>): void {
    for (const [type, sample] of samples) {
        console.log(`\n[${type}] ${JSON.stringify(shapeOf(sample)).slice(0, 1000)}`);
    }
}

function inspectOpenCode(): void {
    let opencodeDbPath: string;
    try {
        opencodeDbPath = resolveOpenCodeDatabasePath();
    } catch (error) {
        console.log(
            `OpenCode DB not found: ${error instanceof Error ? error.message : String(error)}`,
        );
        return;
    }
    console.log(`OpenCode DB: ${basename(opencodeDbPath)}`);

    const db = new Database(opencodeDbPath, { readonly: true });
    // The largest sessions carry the most part variety; the most recently updated carry
    // any type introduced after them. Titles and directories are omitted as session content.
    const sessions = db
        .prepare(`
            SELECT id, message_count, time_updated, source FROM (
                SELECT s.id,
                       (SELECT COUNT(*) FROM message WHERE session_id = s.id) AS message_count,
                       s.time_updated,
                       'largest' AS source
                FROM session s
                ORDER BY message_count DESC
                LIMIT ?
            )
            UNION
            SELECT id, message_count, time_updated, source FROM (
                SELECT s.id,
                       (SELECT COUNT(*) FROM message WHERE session_id = s.id) AS message_count,
                       s.time_updated,
                       'recent' AS source
                FROM session s
                ORDER BY s.time_updated DESC
                LIMIT ?
            )
        `)
        .all(SESSIONS_PER_CRITERION, SESSIONS_PER_CRITERION) as Array<{
        id: string;
        message_count: number;
        time_updated: number;
        source: string;
    }>;
    const sessionIds = [...new Set(sessions.map((s) => s.id))];
    console.log(`OpenCode sessions inspected (${sessionIds.length}):`);
    console.table(sessions);
    if (sessionIds.length === 0) return;

    // Counting in SQL covers every part of every selected session; the capped row scan only feeds the samples.
    const countStmt = db.prepare(`
        SELECT CASE
                   WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.type'), '<missing>')
                   ELSE '<invalid>'
               END AS type,
               COUNT(*) AS count
        FROM part
        WHERE session_id = ?
        GROUP BY type
    `);
    const counts = new Map<string, number>();
    for (const sessionId of sessionIds) {
        for (const row of countStmt.all(sessionId) as Array<{ type: string; count: number }>) {
            counts.set(row.type, (counts.get(row.type) ?? 0) + row.count);
        }
    }
    const countRows = [...counts.entries()]
        .map(([type, count]) => ({ type, count }))
        .sort((a, b) => b.count - a.count);
    console.log(
        `Part type counts across ${sessionIds.length} sessions (all ${countRows.reduce((n, c) => n + c.count, 0)} parts):`,
    );
    console.table(countRows);

    const sampleStmt = db.prepare(
        "SELECT data FROM part WHERE session_id = ? ORDER BY time_created, id LIMIT ?",
    );
    const samples = new Map<string, unknown>();
    let scanned = 0;
    for (const sessionId of sessionIds) {
        const rows = sampleStmt.all(sessionId, SAMPLE_SCAN_LIMIT_PER_SESSION) as Array<{
            data: string;
        }>;
        scanned += rows.length;
        for (const row of rows) {
            const parsed = parseEntry(row.data);
            if (!parsed) continue;
            const type = parsed.type ?? "<missing>";
            if (!samples.has(type)) samples.set(type, parsed);
        }
    }
    const unsampled = countRows
        .map((c) => c.type)
        .filter((t) => t !== "<invalid>" && !samples.has(t));
    console.log(
        `Part shapes (from the first ${SAMPLE_SCAN_LIMIT_PER_SESSION} parts of each session, ${scanned} scanned${unsampled.length > 0 ? `; no sample for: ${unsampled.join(", ")}` : ""}):`,
    );
    printSamples(samples);
}

function walkJsonlFiles(dir: string, out: string[] = []): string[] {
    if (!existsSync(dir)) return out;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) walkJsonlFiles(path, out);
        else if (entry.isFile() && entry.name.endsWith(".jsonl")) out.push(path);
    }
    return out;
}

const PI_FILE_LIMIT = 20;

/** Pi rotates and deletes session files while the inspector runs, so a vanished path is skipped rather than fatal. */
function ignoringVanished<T>(read: () => T): T | undefined {
    try {
        return read();
    } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
        throw error;
    }
}

function inspectPi(): void {
    const all = walkJsonlFiles(piSessionsDir)
        .flatMap((path) => {
            const stat = ignoringVanished(() => statSync(path));
            return stat ? [{ path, mtimeMs: stat.mtimeMs }] : [];
        })
        .sort((a, b) => b.mtimeMs - a.mtimeMs);
    const files = all.slice(0, PI_FILE_LIMIT);
    console.log(
        `Pi JSONL files (${files.length} most recent of ${all.length}${all.length > files.length ? "; older files not inspected" : ""}):`,
    );
    // Session directories under `~/.pi/agent/sessions` are named after project paths, so only the
    // file name and modification time are printed.
    console.table(
        files.map((f, index) => ({
            file: `#${index + 1}`,
            name: basename(f.path),
            modified: new Date(f.mtimeMs).toISOString(),
        })),
    );
    for (const [index, { path: file }] of files.entries()) {
        const label = `#${index + 1} ${basename(file)}`;
        const contents = ignoringVanished(() => readFileSync(file, "utf-8"));
        if (contents === undefined) {
            console.log(`\n${label}\nRemoved before it could be read; skipped`);
            continue;
        }
        const lines = contents.trim().split("\n").filter(Boolean);
        const counts = new Map<string, number>();
        const samples = new Map<string, unknown>();
        let malformed = 0;
        for (const line of lines) {
            // A live session may still be appending its last line, and one damaged
            // entry must not stop the inspection of the remaining files.
            const parsed = parseEntry(line);
            if (!parsed) {
                malformed++;
                continue;
            }
            const type = parsed.type ?? "<missing>";
            counts.set(type, (counts.get(type) ?? 0) + 1);
            if (!samples.has(type)) samples.set(type, parsed);
        }
        console.log(`\n${label}`);
        console.log(
            `Entry types: ${[...counts.entries()].map(([type, count]) => `${type} (${count})`).join(", ")}`,
        );
        if (malformed > 0) console.log(`Skipped ${malformed} malformed line(s)`);
        printSamples(samples);
    }
}

inspectOpenCode();
inspectPi();
