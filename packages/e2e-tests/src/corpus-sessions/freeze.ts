import type { Database } from "bun:sqlite";

import { CANARY } from "./workspace.ts";

const { renameSync, rmSync } = process.getBuiltinModule("node:fs");

/** The pinned harness closures the frozen sessions come from. */
export const OPENCODE_CLOSURE = "opencode-1.18.22";
export const PI_CLOSURE = "pi-0.80.2";

/** `reported` holds `opencode --version` output or a version recorded in a store manifest. */
export function requireOpenCodeClosure(reported: string): string {
    const pinned = OPENCODE_CLOSURE.slice("opencode-".length);
    const found = /(?<![\d.])\d+\.\d+\.\d+(?![\d.])/.exec(reported)?.[0];
    if (found !== pinned) {
        throw new Error(
            `${OPENCODE_CLOSURE} fixtures need OpenCode ${pinned}; found ${found ?? `no version in ${JSON.stringify(reported)}`}`,
        );
    }
    return found;
}

/** A decoded JSON value. */
type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };
type Json = { [key: string]: JsonValue };

function parse(text: string, at: string): JsonValue {
    try {
        return JSON.parse(text) as JsonValue;
    } catch (error) {
        throw new Error(`${at}: not JSON: ${String(error)}`);
    }
}

/** Replaces every scratch path and identifier the run chose with a fixed spelling, recursively. */
function rewrite(value: JsonValue, replace: (text: string) => string): JsonValue {
    if (typeof value === "string") return replace(value);
    if (Array.isArray(value)) return value.map((item) => rewrite(item, replace));
    if (value && typeof value === "object") {
        return Object.fromEntries(
            Object.entries(value).map(([key, member]) => [key, rewrite(member, replace)]),
        );
    }
    return value;
}

/**
 * Marks with `"canary": true` every object whose unit content (`text`, or a tool part's
 * `state.output` or `state.error`) holds the canary, so the redaction test expects it positive.
 */
function markCanaries(value: JsonValue): JsonValue {
    if (Array.isArray(value)) return value.map(markCanaries);
    if (!value || typeof value !== "object") return value;
    const object: Json = Object.fromEntries(
        Object.entries(value).map(([key, member]) => [key, markCanaries(member)]),
    );
    const member = object.state;
    const state =
        member && typeof member === "object" && !Array.isArray(member) ? member : undefined;
    const content = [object.text, state?.output, state?.error];
    if (content.some((text) => typeof text === "string" && text.includes(CANARY))) {
        object.canary = true;
    }
    return object;
}

/** A stable replacement for each distinct match of `pattern`, numbered in sorted order. */
function renamer(texts: readonly string[], pattern: RegExp, prefix: string) {
    const found = new Set<string>();
    for (const text of texts) for (const match of text.matchAll(pattern)) found.add(match[0]);
    const names = new Map(
        [...found]
            .sort()
            .map((id, index) => [id, `${prefix}${String(index + 1).padStart(4, "0")}`]),
    );
    return (text: string) => text.replace(pattern, (id) => names.get(id) ?? id);
}

/** A scratch directory and the fixed path that stands in for it in a fixture. */
export interface ScratchRoot {
    readonly path: string;
    readonly as: string;
}

function scrubber(scratchRoots: readonly ScratchRoot[], texts: readonly string[]) {
    const renames = [
        renamer(texts, /\bses_[0-9A-Za-z]{20,}\b/g, "ses_corpus"),
        renamer(texts, /\bmsg_[0-9A-Za-z]{20,}\b/g, "msg_corpus"),
        renamer(texts, /\bprt_[0-9A-Za-z]{20,}\b/g, "prt_corpus"),
        renamer(texts, /\btool_[0-9a-f]{12}[0-9A-Za-z]{12,}\b/g, "tool_corpus"),
    ];
    const roots = [...scratchRoots].sort((a, b) => b.path.length - a.path.length);
    return (text: string) => {
        let out = text;
        for (const root of roots) out = out.split(root.path).join(root.as);
        for (const rename of renames) out = rename(out);
        return out;
    };
}

function stable(value: JsonValue): string {
    return `${JSON.stringify(value, null, 2)}\n`;
}

/**
 * The frozen form of one OpenCode session read from `opencode.db`: every `message` row with its
 * `part` rows in `(time_created, id)` order, `data` decoded, scratch paths and generated ids
 * replaced, canaries marked. The same store yields the same bytes. A session without a message
 * row is refused: it is what a stale `store.json` names after an interrupted `--run`, and
 * freezing it would overwrite the committed fixture with an empty mix.
 */
export function freezeOpenCode(
    db: Database,
    sessionId: string,
    scratchRoots: readonly ScratchRoot[],
): string {
    const messages = db
        .prepare(
            "SELECT id, time_created, data FROM message WHERE session_id = ? ORDER BY time_created, id",
        )
        .all(sessionId) as { id: string; time_created: number; data: string }[];
    if (messages.length === 0) {
        throw new Error(`opencode.db holds no message for session ${sessionId}`);
    }
    const parts = db.prepare(
        "SELECT id, time_created, data FROM part WHERE session_id = ? AND message_id = ? ORDER BY time_created, id",
    );
    const raw = messages.map((message) => ({
        id: message.id,
        time_created: message.time_created,
        data: parse(message.data, `message ${message.id}`),
        parts: (
            parts.all(sessionId, message.id) as { id: string; time_created: number; data: string }[]
        ).map((part) => ({
            id: part.id,
            time_created: part.time_created,
            data: parse(part.data, `part ${part.id}`),
        })),
    }));
    const scrub = scrubber(scratchRoots, [sessionId, JSON.stringify(raw)]);
    return stable({
        schema: "eidnara.source-corpus/v1",
        harness: "opencode",
        closure: OPENCODE_CLOSURE,
        session_id: scrub(sessionId),
        messages: markCanaries(rewrite(raw, scrub)),
    });
}

/**
 * The frozen form of one Pi session file: every JSONL entry decoded, scratch paths and the
 * session and entry ids replaced, canaries marked. The same file yields the same bytes.
 */
export function freezePi(jsonl: string, scratchRoots: readonly ScratchRoot[]): string {
    const entries = jsonl
        .split("\n")
        .filter((line) => line.trim() !== "")
        .map((line, index) => parse(line, `line ${index + 1}`) as Json);
    const ids = entries.map((entry) => String(entry.id));
    const names = new Map(
        ids.map((id, index) => [
            id,
            index === 0 ? "pi-corpus-session" : `e${String(index).padStart(4, "0")}`,
        ]),
    );
    const paths = scrubber(scratchRoots, []);
    const scrub = (text: string) => names.get(text) ?? paths(text);
    return stable({
        schema: "eidnara.source-corpus/v1",
        harness: "pi",
        closure: PI_CLOSURE,
        session_id: "pi-corpus-session",
        entries: markCanaries(rewrite(entries, scrub)),
    });
}

/** The JSONL a frozen Pi fixture decodes back to, for the Pi reader. */
export function frozenPiJsonl(fixture: string): string {
    const frozen = parse(fixture, "fixture") as { entries: unknown[] };
    return frozen.entries.map((entry) => JSON.stringify(entry)).join("\n");
}

/**
 * Writes a compacted copy of `db` to `target`, replacing any existing file. `VACUUM INTO` writes
 * only to an absent or empty file, so the copy lands in a staging file that is renamed over
 * `target`.
 */
export function snapshotStore(db: Database, target: string): void {
    const staging = `${target}.staging`;
    rmSync(staging, { force: true });
    db.prepare("VACUUM INTO ?").run(staging);
    renameSync(staging, target);
}

interface FrozenOpenCode {
    session_id: string;
    messages: {
        id: string;
        time_created: number;
        data: JsonValue;
        parts: { id: string; time_created: number; data: JsonValue }[];
    }[];
}

function member(value: JsonValue, key: string, at: string): JsonValue {
    if (value === null || typeof value !== "object" || Array.isArray(value) || !(key in value)) {
        throw new Error(`${at}: no \`${key}\``);
    }
    return value[key] as JsonValue;
}

function list(value: JsonValue, at: string): JsonValue[] {
    if (!Array.isArray(value)) throw new Error(`${at}: not an array`);
    return value;
}

function textOf(value: JsonValue, at: string): string {
    if (typeof value !== "string") throw new Error(`${at}: not a string`);
    return value;
}

/** Checks the frozen OpenCode shape field by field: a fixture the freeze did not write fails here. */
function decodeFrozenOpenCode(value: JsonValue): FrozenOpenCode {
    const created = (row: JsonValue, at: string): number => {
        const value = member(row, "time_created", at);
        if (typeof value !== "number") throw new Error(`${at}: time_created is not a number`);
        return value;
    };
    const messages = list(member(value, "messages", "fixture"), "messages").map((row, index) => {
        const at = `messages/${index}`;
        return {
            id: textOf(member(row, "id", at), `${at}/id`),
            time_created: created(row, at),
            data: member(row, "data", at),
            parts: list(member(row, "parts", at), `${at}/parts`).map((part, partIndex) => {
                const partAt = `${at}/parts/${partIndex}`;
                return {
                    id: textOf(member(part, "id", partAt), `${partAt}/id`),
                    time_created: created(part, partAt),
                    data: member(part, "data", partAt),
                };
            }),
        };
    });
    return { session_id: textOf(member(value, "session_id", "fixture"), "session_id"), messages };
}

/**
 * Loads a frozen OpenCode fixture into `db` as `message` and `part` rows, so the SQL reader reads
 * it as it reads a live store. Returns the fixture's session id.
 */
export function loadFrozenOpenCode(db: Database, fixture: string): string {
    const frozen = decodeFrozenOpenCode(parse(fixture, "fixture"));
    db.run(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
    );
    db.run(
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
    );
    const message = db.prepare("INSERT INTO message VALUES (?, ?, ?, ?, ?)");
    const part = db.prepare("INSERT INTO part VALUES (?, ?, ?, ?, ?, ?)");
    for (const row of frozen.messages) {
        message.run(
            row.id,
            frozen.session_id,
            row.time_created,
            row.time_created,
            JSON.stringify(row.data),
        );
        for (const p of row.parts) {
            part.run(
                p.id,
                row.id,
                frozen.session_id,
                p.time_created,
                p.time_created,
                JSON.stringify(p.data),
            );
        }
    }
    return frozen.session_id;
}
