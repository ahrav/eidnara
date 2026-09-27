/**
 * The stale-preference harness driver. It lives the evaluator's fact world through
 * `opencode serve` with the built Eidnara plugin and the daemon's direct-host fixture. Each
 * world turn is the user's prompt, answered by the mock provider with the world's
 * acknowledgement. The context pressure the mock reports rises every turn, so the daemon's own
 * summarizer folds the older history into segments. After the session, every pair's question is
 * asked as a turn of its own, and the provider request OpenCode sends for it is captured whole:
 * the system prompt with the Eidnara guidance, the served `<session-history>` and m1 delta, the
 * raw tail, the auto-search hint, and the tool definitions. `eval_runner stale-arms` turns the
 * capture into the M0 export.
 */

import { Database } from "bun:sqlite";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { RustTestHarness } from "./rust-harness";

export const STALE_CAPTURE_SCHEMA = "eval-stale-capture/v1";

export interface FactTurn {
    user: string;
    assistant: string;
}

export interface FactPair {
    task: string;
    subject: string;
    key: string;
    stale_value: string;
    live_value: string;
    stale_statement: string;
    restatement: string;
    stale_turn: number;
    restating_turn: number;
    question: string;
}

export interface FactWorld {
    schema: string;
    root_seed: string;
    turns: FactTurn[];
    pairs: FactPair[];
}

export interface SegmentTiers {
    start_message: number;
    end_message: number;
    p1: string | null;
    p2: string | null;
    p3: string | null;
    p4: string | null;
}

export interface StaleCapture {
    schema: string;
    harness: "opencode";
    /** The model that wrote the segments, or `fixture/scripted`. */
    summarizer: string;
    world: FactWorld;
    segments: SegmentTiers[];
    requests: Record<string, unknown>;
}

export interface StaleDriverOptions {
    /** The context limit OpenCode's mock model declares. */
    modelContextLimit: number;
    /** Input tokens the mock reports per turn, up to `pressureCeiling` of the limit. */
    tokensPerTurn: number;
    pressureCeiling: number;
    /**
     * The Bedrock model that writes the daemon's segments, through
     * `scripts/bedrock-summarizer.ts`; unset, the fixture's scripted summarizer writes each
     * segment's `p1` and `p2` as the presented lines and `p3` as the range, which serves no prose
     * once a segment decays past P2.
     */
    summarizerModel?: string;
    /** JSONL path for every daemon summarizer request, when gate B needs it. */
    summarizerDump?: string;
    /**
     * The daemon's cache TTL and the idle gap after the session. The gap is longer than the TTL,
     * so the first question turn is a HARD pass that re-freezes m0 over every published segment,
     * as when a user comes back to a long session.
     */
    cacheTtl: string;
    idleMs: number;
    /** Called after each world turn and each question, for progress. */
    progress?: (done: number, total: number) => void;
}

export const DEFAULT_STALE_DRIVER: StaleDriverOptions = {
    modelContextLimit: 200_000,
    tokensPerTurn: 1_500,
    pressureCeiling: 0.9,
    cacheTtl: "30s",
    idleMs: 40_000,
};

function usage(options: StaleDriverOptions, turn: number) {
    const ceiling = Math.floor(options.modelContextLimit * options.pressureCeiling);
    return {
        input_tokens: Math.min(ceiling, options.tokensPerTurn * (turn + 1)),
        output_tokens: 20,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    };
}

/** The mock's reply to a question turn, which the driver reverts. */
const QUESTION_REPLY = "Let me check.";

/** Waits until no summarizer firing is in flight for the session; a real model can take minutes. */
async function quiesce(h: RustTestHarness, sessionId: string): Promise<void> {
    let quiet = 0;
    const deadline = Date.now() + 900_000;
    let summarizer: Record<string, unknown> = {};
    while (quiet < 3) {
        if (Date.now() > deadline) {
            throw new Error(
                `the history summarizer did not settle: ${JSON.stringify(summarizer).slice(0, 2_000)}`,
            );
        }
        const status = await h.host.primaryStatus(sessionId, h.env.workdir);
        if (!status.history_summarizer || typeof status.history_summarizer !== "object") {
            throw new Error("session status carries no history_summarizer block");
        }
        summarizer = status.history_summarizer as Record<string, unknown>;
        quiet =
            summarizer.fired_at_ms == null && summarizer.producer_run_id == null ? quiet + 1 : 0;
        await Bun.sleep(200);
    }
}

/** The daemon's stored segments, read from its store after the session. */
function storedSegments(dataDir: string): SegmentTiers[] {
    const path = join(dataDir, "eidnara", "context", "store.db");
    if (!existsSync(path)) throw new Error(`no daemon store at ${path}`);
    const db = new Database(path, { readonly: true });
    try {
        return db
            .query(
                "SELECT start_message, end_message, p1, p2, p3, p4 FROM history_segments ORDER BY sequence",
            )
            .all() as SegmentTiers[];
    } finally {
        db.close();
    }
}

function lastUserText(body: { messages?: Array<{ role: string; content: unknown }> }): string {
    const last = body.messages?.filter((m) => m.role === "user").at(-1);
    return JSON.stringify(last?.content ?? "");
}

/**
 * Reverts the session's newest user message and the reply after it, as a user's undo does.
 * OpenCode marks the session and drops the reverted turn when the next prompt arrives, so the
 * caller checks that the next captured request no longer carries it.
 */
async function revertLastTurn(h: RustTestHarness, sessionId: string): Promise<void> {
    const userIds = (await h.listMessages(sessionId))
        .map((message) => message.info)
        .filter((info) => info?.role === "user" && info.id)
        .map((info) => info?.id as string);
    const last = userIds.at(-1);
    if (!last) throw new Error("no user message to revert");
    await h.revertMessage(sessionId, last);
}

/** Lives `world` through the real harness stack and captures every question turn's request. */
export async function captureStaleWorld(
    world: FactWorld,
    options: StaleDriverOptions = DEFAULT_STALE_DRIVER,
): Promise<StaleCapture> {
    const h = await RustTestHarness.create({
        modelContextLimit: options.modelContextLimit,
        eidnaraConfig: {
            history_summarizer: { model: "fixture/deterministic" },
            cache_ttl: options.cacheTtl,
        },
        // Set explicitly, so a variable exported in the caller's shell never changes a run.
        daemonEnv: {
            EIDNARA_FIXTURE_SUMMARIZER_COMMAND: options.summarizerModel
                ? resolve(import.meta.dir, "../scripts/bedrock-summarizer.ts")
                : "",
            EIDNARA_STALE_SUMMARIZER_MODEL: options.summarizerModel ?? "",
            EIDNARA_FIXTURE_SUMMARIZER_DUMP: options.summarizerDump
                ? resolve(options.summarizerDump)
                : "",
        },
    });
    try {
        const sessionId = await h.createSession();
        const total = world.turns.length + world.pairs.length;
        let done = 0;
        for (const [index, turn] of world.turns.entries()) {
            h.mock.setDefault({ text: turn.assistant, usage: usage(options, index) });
            await h.sendPrompt(sessionId, turn.user);
            options.progress?.(++done, total);
        }
        await quiesce(h, sessionId);
        await Bun.sleep(options.idleMs);
        const requests: Record<string, unknown> = {};
        let previous: string | undefined;
        for (const pair of world.pairs) {
            // The reply carries no value; the revert below removes it before the next question.
            h.mock.setDefault({ text: QUESTION_REPLY, usage: usage(options, world.turns.length) });
            await h.sendPrompt(sessionId, pair.question);
            const body = h.mainRequests().at(-1)?.body;
            if (!body || !lastUserText(body).includes(pair.question)) {
                throw new Error(`no captured request for ${pair.task}'s question`);
            }
            const messages = JSON.stringify(body.messages);
            if (previous && (messages.includes(previous) || messages.includes(QUESTION_REPLY))) {
                throw new Error(`${pair.task}'s request still carries the reverted turn`);
            }
            previous = pair.question;
            requests[pair.task] = body;
            options.progress?.(++done, total);
            // Every question is asked of the session as it stood after the world: the turn is
            // reverted through OpenCode before the next, so no question sees another's turn.
            await revertLastTurn(h, sessionId);
            await quiesce(h, sessionId);
        }
        return {
            schema: STALE_CAPTURE_SCHEMA,
            harness: "opencode",
            summarizer: options.summarizerModel ?? "fixture/scripted",
            world,
            segments: storedSegments(h.env.dataDir),
            requests,
        };
    } finally {
        await h.dispose();
    }
}
