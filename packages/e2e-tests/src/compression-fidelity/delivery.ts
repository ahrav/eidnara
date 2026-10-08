/**
 * Drives compression fidelity cases through the actual OpenCode invocation.
 *
 * The direct-host fixture owns the corpus: it resolves scenario IDs, returns a source's native
 * records and leak probes, and binds the approved example to the ordinals each summarizer request
 * presents. This module seeds those records into OpenCode's own `opencode.db` after the session's
 * newest message, reads publication from the daemon's session status, and judges a retained
 * provider capture against the reviewed tier bodies. It never reads native text from the corpus
 * file.
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

/**
 * The fixture-authored assistant turn seeded after a source that ends on a user message, so the
 * next prompt presents as its own record instead of merging into the source's last one.
 */
const SEEDED_CLOSE = "Noted; I will hold here until the next request.";

/** The reviewed bodies a served segment is matched against, P1 first. */
const REVIEWED_TIERS = ["p1", "p2", "p3"] as const;

/** One planned OpenCode `message` row and its `part` rows. */
export interface SeedRow {
    id: string;
    created: number;
    completed: number;
    data: Json;
    parts: { id: string; at: number; data: Json }[];
}

/** The newest message data of each role, whose fields the seeded rows inherit. */
export interface SeedTemplates {
    user: Json;
    assistant: Json;
}

function collapse(text: string): string {
    return text.split(/\s+/).filter(Boolean).join(" ");
}

/** The title, importance, and P1-P3 bodies of a reviewed output's case segment. */
export function reviewedTiers(reviewedOutput: string): {
    title: string;
    importance: number;
    bodies: string[];
} {
    const title = /title="([^"]*)"/.exec(reviewedOutput)?.[1];
    if (title === undefined) throw new Error("reviewed output has no title");
    const importance = /importance="(\d+)"/.exec(reviewedOutput)?.[1];
    if (importance === undefined) throw new Error("reviewed output has no importance");
    const bodies = REVIEWED_TIERS.map((tag) => {
        const body = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`).exec(reviewedOutput)?.[1];
        if (body === undefined) throw new Error(`reviewed output has no ${tag}`);
        return body;
    });
    return { title, importance: Number(importance), bodies };
}

/**
 * Plans `source`'s native records as OpenCode rows after `latest`, one millisecond apart, under
 * OpenCode-shaped IDs, followed by a fixture-authored assistant turn when the source ends on a
 * user message. Each message inherits the fields of its role's template.
 */
export function planSeed(
    source: ScriptSource,
    templates: SeedTemplates,
    latest: number,
): SeedRow[] {
    const records = [...source.messages];
    if (records.at(-1)?.info.role === "user") {
        records.push({
            info: { role: "assistant" },
            parts: [{ id: "close", type: "text", text: SEEDED_CLOSE }],
        });
    }
    let clock = latest + 1;
    let parentId: string | null = null;
    return records.map((native, index) => {
        const role = native.info.role;
        const created = clock;
        const completed = created + native.parts.length + 1;
        clock = completed + 1;
        const identity = `${source.source}-${index}`;
        const id = generateMessageId(created, 1n, identity);
        if (role === "user") parentId = id;
        const data: Json = {
            ...templates[role],
            time: role === "user" ? { created } : { created, completed },
            ...(role === "assistant" && parentId ? { parentID: parentId } : {}),
        };
        const parts = native.parts.map((part, k) => {
            const at = created + k + 1;
            const { id: _nativeId, ...fields } = part;
            const state = part.state as Json | undefined;
            return {
                id: generatePartId(at, 2n, `${identity}-${k}`),
                at,
                data:
                    part.type === "tool"
                        ? {
                              ...fields,
                              state: {
                                  ...state,
                                  title: "",
                                  metadata: {},
                                  time: { start: at, end: at },
                              },
                          }
                        : fields,
            };
        });
        return { id, created, completed, data, parts };
    });
}

/**
 * Inserts {@link planSeed}'s rows into the session after its newest message. Call after at least
 * one completed turn, which supplies the templates, and restart OpenCode before the next prompt.
 */
export function seedSource(
    harness: RustTestHarness,
    sessionId: string,
    source: ScriptSource,
): SeedRow[] {
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
        const template = (role: "user" | "assistant"): Json => {
            const row = db
                .prepare(
                    "SELECT data FROM message WHERE session_id = ? AND json_extract(data, '$.role') = ? ORDER BY time_created DESC LIMIT 1",
                )
                .get(sessionId, role) as { data: string } | undefined;
            if (!row) throw new Error(`seeding ${source.source} requires a prior ${role} message`);
            return JSON.parse(row.data) as Json;
        };
        const rows = planSeed(
            source,
            { user: template("user"), assistant: template("assistant") },
            latest,
        );
        const insertMessage = db.prepare(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
        );
        const insertPart = db.prepare(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?)",
        );
        db.transaction(() => {
            for (const row of rows) {
                insertMessage.run(
                    row.id,
                    sessionId,
                    row.created,
                    row.completed,
                    JSON.stringify(row.data),
                );
                for (const part of row.parts) {
                    insertPart.run(
                        part.id,
                        row.id,
                        sessionId,
                        part.at,
                        part.at,
                        JSON.stringify(part.data),
                    );
                }
            }
        })();
        return rows;
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

/** The text content of one provider message or system field. */
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
 * The reviewed tier at which a request serves the case segment titled `title`: the body it
 * carries, the heading alone for P4, or absence for P5. A body matching no reviewed tier is
 * `unmatched`.
 */
export function servedTier(
    texts: readonly string[],
    title: string,
    bodies: readonly string[],
): ServedTier | "unmatched" {
    const text = texts.find((candidate) => candidate.includes(title));
    if (text === undefined) return "p5";
    // A segment runs from its heading to the next heading or the wrapper's closing tag.
    const segment = text.slice(text.indexOf(title)).split(/\n(?:## |<\/)/)[0] ?? "";
    const carried = bodies.findIndex((body) => segment.includes(body));
    const tier = REVIEWED_TIERS[carried];
    if (tier) return tier;
    return segment.trim().split("\n").length === 1 ? "p4" : "unmatched";
}

/** The m0 history wrapper and the m1 window's wrapper of rows published since. */
const M0_WRAPPER = /<session-history>[\s\S]*?<\/session-history>/;
const M1_WRAPPER = /<session-history-since>[\s\S]*?<\/session-history-since>/;

/** Where served texts carry the case segment titled `title`. */
export function stageOf(texts: readonly string[], title: string): "m1" | "m0" | "absent" {
    for (const text of texts) {
        if (M1_WRAPPER.exec(text)?.[0].includes(title)) return "m1";
        if (M0_WRAPPER.exec(text)?.[0].includes(title)) return "m0";
    }
    return "absent";
}

/** Leak probes found in `texts` outside both history wrappers. */
export function leaksOutside(texts: readonly string[], probes: readonly string[]): string[] {
    const outside = texts
        .map((text) =>
            collapse(
                text
                    .replace(new RegExp(M0_WRAPPER, "g"), "")
                    .replace(new RegExp(M1_WRAPPER, "g"), ""),
            ),
        )
        .join("\n");
    return probes.filter((probe) => outside.includes(probe));
}

/** Leak probes in the request's system text or any message, outside the history wrappers. */
export function rawTailLeaks(capture: RetainedCapture, probes: readonly string[]): string[] {
    const system = capture.request.body.system;
    return leaksOutside(
        [
            typeof system === "string" ? system : messageText({ content: system }),
            ...captureTexts(capture),
        ],
        probes,
    );
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
    leakProbes: readonly string[];
}

export interface DeliveryVerdict {
    refusals: DeliveryRefusal[];
    tier: ServedTier | "unmatched" | null;
    leaks: string[];
}

/**
 * Judges one provider capture for compression credit: a captured nonempty request, an applied
 * recipe served from the transform, the case segment at a reviewed tier, and covered native text
 * only inside the history wrappers. Each failed precondition is its own refusal; P5 absence is
 * `history_absent`, which a scenario expecting omission reads as its result.
 */
export function judgeDelivery(input: DeliveryInput): DeliveryVerdict {
    const refusals: DeliveryRefusal[] = [];
    if (!input.pass) refusals.push("missing_pass");
    else if (!input.pass.applied) refusals.push("not_applied");
    else if (input.pass.servedFrom !== "transform") refusals.push("raw_pass_through");
    if (!input.capture) {
        refusals.push("missing_capture");
        return { refusals, tier: null, leaks: [] };
    }
    const texts = captureTexts(input.capture);
    if (texts.length === 0) {
        refusals.push("empty_capture");
        return { refusals, tier: null, leaks: [] };
    }
    const tier = servedTier(texts, input.title, input.bodies);
    if (tier === "p5") refusals.push("history_absent");
    if (tier === "unmatched") refusals.push("body_unmatched");
    const leaks = rawTailLeaks(input.capture, input.leakProbes);
    if (leaks.length > 0) refusals.push("raw_tail_leak");
    return { refusals, tier, leaks };
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
