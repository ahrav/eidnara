/**
 * Drives compression fidelity cases through the actual OpenCode invocation.
 *
 * The direct-host fixture owns the corpus: it resolves scenario IDs, returns a source's native
 * records, and binds the approved example to the ordinals each summarizer request presents. This
 * module seeds those records into OpenCode's own `opencode.db` after the session's newest message,
 * reads publication from the daemon's session status, and judges a retained provider capture
 * against the reviewed tier bodies. It never reads native text from the corpus file.
 */

import { Database } from "bun:sqlite";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import {
    generateMessageId,
    generatePartId,
} from "@eidnara/opencode/features/context/compaction-marker";
import { publishJsonAtomically } from "../atomic-publish";
import type { RetainedCapture, RustPassLine, RustTestHarness } from "../rust-harness";
import type { ScriptSource } from "../rust-runner/hermetic-host";
import { COMPRESSION_FIDELITY_CORPUS_SHA256, type ServedTier } from "./corpus";

type Json = Record<string, unknown>;

/** Characters of a native text block that locate it in a provider request. */
const PROBE_CHARS = 48;

/**
 * The fixture-authored assistant turn seeded after a source that ends on a user message, so the
 * next prompt presents as its own record instead of merging into the source's last one.
 */
export const SEEDED_CLOSE = "Noted; I will hold here until the next request.";

/** One seeded native text block and the probe that finds it in a provider request. */
export interface SeededSource {
    source: string;
    messageIds: string[];
    /** The leading characters of every native text block, whitespace collapsed. */
    probes: string[];
}

function collapse(text: string): string {
    return text.split(/\s+/).filter(Boolean).join(" ");
}

/** The first `<history_segment>` of a reviewed output: its title and P1-P3 bodies. */
export function reviewedTiers(reviewedOutput: string): { title: string; bodies: string[] } {
    const title = /title="([^"]*)"/.exec(reviewedOutput)?.[1];
    if (title === undefined) throw new Error("reviewed output has no title");
    const bodies = ["p1", "p2", "p3"].map((tag) => {
        const body = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`).exec(reviewedOutput)?.[1];
        if (body === undefined) throw new Error(`reviewed output has no ${tag}`);
        return body;
    });
    return { title, bodies };
}

/**
 * Inserts `source`'s native records into the session after its newest message, one millisecond
 * apart, under OpenCode-shaped IDs, followed by {@link SEEDED_CLOSE} when the source ends on a
 * user message. Message and part fields beyond the corpus's come from the session's newest
 * message of the same role, so call after at least one completed turn and restart OpenCode
 * before the next prompt.
 */
export function seedSource(
    harness: RustTestHarness,
    sessionId: string,
    source: ScriptSource,
): SeededSource {
    const db = new Database(join(harness.env.dataDir, "opencode", "opencode.db"));
    try {
        db.exec("PRAGMA busy_timeout = 30000");
        const latest = (
            db
                .prepare(
                    "SELECT COALESCE(MAX(time_created), 0) AS latest FROM message WHERE session_id = ?",
                )
                .get(sessionId) as { latest: number }
        ).latest;
        const template = (role: string): Json => {
            const row = db
                .prepare(
                    "SELECT data FROM message WHERE session_id = ? AND json_extract(data, '$.role') = ? ORDER BY time_created DESC LIMIT 1",
                )
                .get(sessionId, role) as { data: string } | undefined;
            if (!row) throw new Error(`seeding requires a prior ${role} message`);
            return JSON.parse(row.data) as Json;
        };
        const templates = { user: template("user"), assistant: template("assistant") };
        const insertMessage = db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        );
        const insertPart = db.prepare(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?)",
        );
        const seeded: SeededSource = { source: source.source, messageIds: [], probes: [] };
        let clock = latest + 1;
        let parentId: string | null = null;
        const records = [...source.messages];
        const closing = (records.at(-1)?.info as { role?: string } | undefined)?.role === "user";
        if (closing) {
            records.push({
                info: { role: "assistant" },
                parts: [{ id: "close", type: "text", text: SEEDED_CLOSE }],
            });
        }
        db.transaction(() => {
            for (const [index, native] of records.entries()) {
                const info = native.info as { role: "user" | "assistant" };
                const parts = native.parts as Json[];
                const created = clock;
                const completed = created + parts.length + 1;
                clock = completed + 1;
                const identity = `${source.source}-${index}`;
                const messageId = generateMessageId(created, 1n, identity);
                const data: Json = {
                    ...templates[info.role],
                    time: info.role === "user" ? { created } : { created, completed },
                    ...(info.role === "assistant" && parentId ? { parentID: parentId } : {}),
                };
                insertMessage.run(messageId, sessionId, created, completed, JSON.stringify(data));
                if (info.role === "user") parentId = messageId;
                const authored = index < source.messages.length;
                if (authored) seeded.messageIds.push(messageId);
                for (const [k, part] of parts.entries()) {
                    const at = created + k + 1;
                    const partId = generatePartId(at, 2n, `${identity}-${k}`);
                    const { id: _id, ...fields } = part;
                    if (authored && part.type === "text") {
                        seeded.probes.push(collapse(String(part.text)).slice(0, PROBE_CHARS));
                    }
                    const partData =
                        part.type === "tool"
                            ? {
                                  ...fields,
                                  state: {
                                      ...(part.state as Json),
                                      title: "",
                                      metadata: {},
                                      time: { start: at, end: at },
                                  },
                              }
                            : fields;
                    insertPart.run(partId, messageId, sessionId, at, at, JSON.stringify(partData));
                }
            }
        })();
        return seeded;
    } finally {
        db.close();
    }
}

/** The daemon's published history-segment count for the session. */
export async function publishedCount(harness: RustTestHarness, sessionId: string): Promise<number> {
    const status = await harness.host.primaryStatus(
        sessionId,
        harness.env.workdir,
        "session.status",
    );
    const summarizer = status.history_summarizer as
        | { counters?: { published?: number } }
        | undefined;
    return summarizer?.counters?.published ?? 0;
}

/** The text content of one provider message. */
export function messageText(message: { content?: unknown }): string {
    const content = message.content;
    if (typeof content === "string") return content;
    if (!Array.isArray(content)) return "";
    return content
        .map((block) => {
            const value = block as { type?: string; text?: string; content?: unknown };
            if (typeof value.text === "string") return value.text;
            if (value.type === "tool_result") return messageText({ content: value.content });
            return "";
        })
        .join("\n");
}

/** The messages of a capture, as text, in wire order. */
export function captureTexts(capture: RetainedCapture): string[] {
    return (capture.request.body.messages ?? []).map(messageText);
}

/**
 * Where a delivered request serves the case segment titled `title`: the index of the message
 * carrying its heading, and the reviewed tier its body matches. P4 is the heading alone, P5 is
 * absence. A body matching no reviewed tier is reported as `unmatched`.
 */
export function servedTier(
    texts: readonly string[],
    title: string,
    bodies: readonly string[],
): { tier: ServedTier | "unmatched"; message: number } {
    const message = texts.findIndex((text) => text.includes(title));
    if (message < 0) return { tier: "p5", message };
    const text = texts[message] ?? "";
    // A segment runs from its heading to the next heading or the wrapper's closing tag.
    const segment = text.slice(text.indexOf(title)).split(/\n(?:## |<\/)/)[0] ?? "";
    const carried = bodies.findIndex((body) => segment.includes(body));
    if (carried >= 0) return { tier: (["p1", "p2", "p3"] as const)[carried] ?? "p1", message };
    return { tier: segment.trim().split("\n").length === 1 ? "p4" : "unmatched", message };
}

/** Native probes that appear in any message other than the history messages at `history`. */
export function rawTailLeaks(
    texts: readonly string[],
    history: ReadonlySet<number>,
    probes: readonly string[],
): string[] {
    const tail = texts
        .filter((_, index) => !history.has(index))
        .map(collapse)
        .join("\n");
    return probes.filter((probe) => tail.includes(probe));
}

/** Every pass line logged for `sessionId`, oldest first. */
export function sessionPasses(harness: RustTestHarness, sessionId: string): RustPassLine[] {
    return harness.readRustPasses().filter((pass) => pass.raw.includes(`[${sessionId}]`));
}

/** Why one delivery observation earns no compression credit. */
export type DeliveryRefusal =
    | "missing_capture"
    | "empty_capture"
    | "missing_pass"
    | "not_applied"
    | "raw_pass_through"
    | "history_absent"
    | "body_unmatched"
    | "raw_tail_leak";

export interface DeliveryInput {
    capture: RetainedCapture | undefined;
    pass: RustPassLine | undefined;
    title: string;
    bodies: readonly string[];
    probes: readonly string[];
}

export interface DeliveryVerdict {
    refusals: DeliveryRefusal[];
    tier: ServedTier | "unmatched" | null;
    /** Index of the message serving the case segment, or -1 when absent. */
    historyMessage: number;
    leaks: string[];
}

/**
 * Judges one provider capture for compression credit: a captured nonempty request, an applied
 * recipe served from the transform, the case segment at a reviewed tier, and no covered native
 * text outside the history messages. Each failed precondition is its own refusal; P5 absence is
 * reported as `history_absent`, which a scenario expecting omission reads as its result.
 */
export function judgeDelivery(input: DeliveryInput): DeliveryVerdict {
    const refusals: DeliveryRefusal[] = [];
    if (!input.pass) refusals.push("missing_pass");
    else if (!input.pass.applied) refusals.push("not_applied");
    else if (input.pass.servedFrom !== "transform") refusals.push("raw_pass_through");
    if (!input.capture) {
        refusals.push("missing_capture");
        return { refusals, tier: null, historyMessage: -1, leaks: [] };
    }
    const texts = captureTexts(input.capture);
    if (texts.length === 0) {
        refusals.push("empty_capture");
        return { refusals, tier: null, historyMessage: -1, leaks: [] };
    }
    const served = servedTier(texts, input.title, input.bodies);
    if (served.tier === "p5") refusals.push("history_absent");
    if (served.tier === "unmatched") refusals.push("body_unmatched");
    const history = new Set(
        texts.flatMap((text, index) =>
            text.includes("<session-history>") || index === served.message ? [index] : [],
        ),
    );
    const leaks = rawTailLeaks(texts, history, input.probes);
    if (leaks.length > 0) refusals.push("raw_tail_leak");
    return { refusals, tier: served.tier, historyMessage: served.message, leaks };
}

/** The environment variable naming the private observation directory. */
export const OBSERVATIONS_DIR_ENV = "EIDNARA_FIDELITY_OBSERVATIONS_DIR";

/** One owner-attributed delivery observation, in the daemon witnesses' record shape. */
export interface DeliveryObservation {
    case: string;
    source: string;
    scenario: string;
    stage: string;
    terminal: string;
    markers: string[];
    detail: Record<string, unknown>;
}

const OBSERVATION_OWNER = "opencode-delivery";

/**
 * Writes `observation` as `opencode-delivery.<case>.<scenario>.<stage>.json` into the directory
 * {@link OBSERVATIONS_DIR_ENV} names, created owner-only, with an owner-only file renamed into
 * place. Without the variable nothing is written. Returns the path written, if any.
 */
export function emitObservation(
    observation: DeliveryObservation,
    dir: string | undefined = process.env[OBSERVATIONS_DIR_ENV],
): string | undefined {
    if (!dir) return undefined;
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const path = join(
        dir,
        `${OBSERVATION_OWNER}.${observation.case}.${observation.scenario}.${observation.stage}.json`,
    );
    publishJsonAtomically(
        {
            schema_version: 1,
            owner: OBSERVATION_OWNER,
            corpus_sha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
            ...observation,
        },
        path,
        { mode: 0o600 },
    );
    return path;
}

/** Polls `read` until it yields a value, for at most `timeoutMs`. */
export async function waitUntil<T>(
    read: () => Promise<T | null>,
    label: string,
    diagnostics: () => string = () => "",
    timeoutMs = 120_000,
): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const value = await read();
        if (value !== null) return value;
        if (Date.now() >= deadline) {
            throw new Error(`timed out waiting for ${label}\n${diagnostics()}`);
        }
        await Bun.sleep(200);
    }
}
