#!/usr/bin/env bun
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { getDataDir } from "../src/shared/data-path";
import { Database } from "../src/shared/sqlite";

const opencodeDbPath = join(getDataDir(), "opencode", "opencode.db");
const piSessionsDir = join(homedir(), ".pi", "agent", "sessions");

const SHAPE_MAX_DEPTH = 6;

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

function printSamples(samples: Map<string, unknown>): void {
    for (const [type, sample] of samples) {
        console.log(`\n[${type}] ${JSON.stringify(shapeOf(sample)).slice(0, 1000)}`);
    }
}

function inspectOpenCode(): void {
    if (!existsSync(opencodeDbPath)) {
        console.log(`OpenCode DB not found: ${opencodeDbPath}`);
        return;
    }

    const db = new Database(opencodeDbPath, { readonly: true });
    const sessions = db
        .prepare(`
            SELECT id, title, directory,
                   (SELECT COUNT(*) FROM message WHERE session_id = s.id) AS message_count
            FROM session s
            ORDER BY message_count DESC
            LIMIT 5
        `)
        .all();
    console.log("Largest OpenCode sessions:");
    console.table(sessions);

    const sessionId = (sessions[0] as { id?: string } | undefined)?.id;
    if (!sessionId) return;

    // Counting in SQL covers every part; the row scan below is capped and only feeds the samples.
    const counts = db
        .prepare(`
            SELECT CASE
                       WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.type'), '<missing>')
                       ELSE '<invalid>'
                   END AS type,
                   COUNT(*) AS count
            FROM part
            WHERE session_id = ?
            GROUP BY type
            ORDER BY count DESC
        `)
        .all(sessionId) as Array<{ type: string; count: number }>;
    console.log(
        `Part type counts for ${sessionId} (all ${counts.reduce((n, c) => n + c.count, 0)} parts):`,
    );
    console.table(counts);

    const SAMPLE_SCAN_LIMIT = 10000;
    const rows = db
        .prepare("SELECT data FROM part WHERE session_id = ? ORDER BY time_created, id LIMIT ?")
        .all(sessionId, SAMPLE_SCAN_LIMIT) as Array<{ data: string }>;
    const samples = new Map<string, unknown>();
    for (const row of rows) {
        let parsed: { type?: string };
        try {
            parsed = JSON.parse(row.data) as { type?: string };
        } catch {
            continue;
        }
        const type = parsed.type ?? "<missing>";
        if (!samples.has(type)) samples.set(type, parsed);
    }
    const unsampled = counts.map((c) => c.type).filter((t) => t !== "<invalid>" && !samples.has(t));
    console.log(
        `Part shapes (from the first ${rows.length} parts${unsampled.length > 0 ? `; no sample for: ${unsampled.join(", ")}` : ""}):`,
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

function inspectPi(): void {
    const all = walkJsonlFiles(piSessionsDir)
        .map((path) => ({ path, mtimeMs: statSync(path).mtimeMs }))
        .sort((a, b) => b.mtimeMs - a.mtimeMs);
    const files = all.slice(0, PI_FILE_LIMIT).map((f) => f.path);
    console.log(
        `Pi JSONL files (${files.length} most recent of ${all.length}${all.length > files.length ? "; older files not inspected" : ""}):`,
    );
    console.table(files.map((path) => ({ path })));
    for (const file of files) {
        const lines = readFileSync(file, "utf-8").trim().split("\n").filter(Boolean);
        const counts = new Map<string, number>();
        const samples = new Map<string, unknown>();
        let malformed = 0;
        for (const line of lines) {
            // A live session may still be appending its last line, and one damaged
            // entry must not stop the inspection of the remaining files.
            let parsed: { type?: string };
            try {
                parsed = JSON.parse(line) as { type?: string };
            } catch {
                malformed++;
                continue;
            }
            const type = parsed.type ?? "<missing>";
            counts.set(type, (counts.get(type) ?? 0) + 1);
            if (!samples.has(type)) samples.set(type, parsed);
        }
        console.log(`\n${file}`);
        console.log(
            `Entry types: ${[...counts.entries()].map(([type, count]) => `${type} (${count})`).join(", ")}`,
        );
        if (malformed > 0) console.log(`Skipped ${malformed} malformed line(s)`);
        printSamples(samples);
    }
}

inspectOpenCode();
inspectPi();
