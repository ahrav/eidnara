/**
 * Drives fixed-lead arms of the compaction-timing scenario through the mock provider and the direct-host fixture, and reads every measurement back: firing timelines, counters, and ring aggregates from `session.status` through the shared status helper, served action, reason, and emergency wait from the plugin's `rust pass:` lines, and provider input from the usage the mock returned to the plugin.
 *
 * Pressure is either scripted per turn as a percentage of the plugin's context limit, so controlled arms differ only in the lead, or derived from the size of the request the daemon actually served, so a fold lowers it and the sweep's input totals depend on what the daemon did. Either way the mock splits each response's input into cache read and write by how much of the previous main request's message prefix the new request repeats, so cache evidence follows what the daemon served rather than being scripted per action.
 */

import { execFileSync } from "node:child_process";
import type {
    CompactionFiring,
    CompactionTiming,
} from "@eidnara/opencode/shared/compaction-timing";
import { type RustPassLine, RustTestHarness, stableSerialize } from "./rust-harness";

export const EXECUTE_THRESHOLD = 65;
export const PREPARE_LEAD_ENV = "EIDNARA_HISTORY_SUMMARIZER_PREPARE_LEAD";
const MODEL_CONTEXT_LIMIT = 100_000;
const TURN_BALLAST_TOKENS = 900;
const MAX_WAIT_TURNS = 40;
/** Served pressure counts request bytes at this many per token. */
const BYTES_PER_TOKEN = 4;
const CACHE_ACTIONS = ["SOFT+", "SOFT", "HARD"] as const;

/** The raw firing fields the shared helper does not project. */
export interface RawFiring {
    usage?: { usage_percentage?: number };
    outcome?: { sequence?: number | null };
}

export interface TurnRecord {
    pass: RustPassLine | undefined;
    timing: CompactionTiming;
}

export type Pressure = { kind: "scripted" } | { kind: "served" };

export interface ArmReport {
    run_id: string;
    scenario: string;
    /** `-dirty` marks a working tree with uncommitted changes. */
    commit: string;
    lead: number | "default";
    provenance: "mock";
    pressure: Pressure["kind"];
    execute_threshold_percentage: number;
    usage_denominator: "usage_soft_limit_tokens";
    /** `null` when the newest ring entry carries no soft limit. */
    soft_limit_tokens: number | null;
    clocks: {
        emergency_wait_ms: "daemon_monotonic";
        publish_to_activation_ms: "daemon_wall_ms";
    };
    /** Pressure-path firings in the window, the session's first firing excluded. */
    pressure_path_firings: number;
    /** Firings the eight-entry window no longer holds; their durations are unobserved. */
    evicted_firings: number | null;
    censored_firings: number;
    passes: number;
    emergency_passes: number;
    emergency_wait_ms: { max: number | null; ordered: number[] };
    publish_to_activation_ms: { max: number | null; n: number; ordered: number[] };
    hard_passes: number;
    soft_passes: number;
    published: number | null;
    rebuilds_per_publication: number | null;
    validation_rejected: number | null;
    invalidated: number | null;
    superseded_before_activation: number | null;
    total_input_tokens: number;
    total_input_tokens_source: "mock usage returned to the plugin, summed by the harness";
    /** `null` for an action with no paired sample. */
    cache_read_share_after: Record<(typeof CACHE_ACTIONS)[number], number | null>;
    cache_read_samples_after: Record<(typeof CACHE_ACTIONS)[number], number>;
}

function commonPrefixBytes(previous: string, next: string): number {
    const limit = Math.min(previous.length, next.length);
    let index = 0;
    while (index < limit && previous.charCodeAt(index) === next.charCodeAt(index)) index += 1;
    return index;
}

function currentCommit(): string {
    try {
        const head = execFileSync("git", ["rev-parse", "--short", "HEAD"], {
            encoding: "utf8",
        }).trim();
        const dirty = execFileSync("git", ["status", "--porcelain"], { encoding: "utf8" }).trim();
        return dirty ? `${head}-dirty` : head;
    } catch {
        return "unknown";
    }
}

export class TimingSession {
    readonly h: RustTestHarness;
    readonly sessionId: string;
    readonly lead: number | undefined;
    readonly pressure: Pressure;
    private turnCount = 0;
    private pressureTokens = 0;
    private previousMainMessages = "";
    private contextLimit = 0;
    private readonly configuredLimit: number;
    totalInputTokens = 0;

    private constructor(
        h: RustTestHarness,
        sessionId: string,
        lead: number | undefined,
        pressure: Pressure,
        configuredLimit: number,
    ) {
        this.configuredLimit = configuredLimit;
        this.h = h;
        this.sessionId = sessionId;
        this.lead = lead;
        this.pressure = pressure;
    }

    /** `lead` undefined runs the daemon's built-in default, the baseline arm. */
    static async start(
        lead: number | undefined,
        options: { pressure?: Pressure; modelContextLimit?: number } = {},
    ): Promise<TimingSession> {
        const modelContextLimit = options.modelContextLimit ?? MODEL_CONTEXT_LIMIT;
        const h = await RustTestHarness.create({
            modelContextLimit,
            eidnaraConfig: {
                execute_threshold_percentage: EXECUTE_THRESHOLD,
                protected_tags: 1,
                history_summarizer: { model: "fixture/deterministic" },
            },
            daemonEnv: lead === undefined ? {} : { [PREPARE_LEAD_ENV]: String(lead) },
        });
        try {
            await h.host.backendSuccess();
            const session = new TimingSession(
                h,
                await h.createSession(),
                lead,
                options.pressure ?? { kind: "scripted" },
                modelContextLimit,
            );
            h.mock.addMatcher((body) => session.respond(body));
            return session;
        } catch (error) {
            await h.dispose().catch(() => undefined);
            throw error;
        }
    }

    /** Main requests carry the Eidnara system block; everything else falls through to the default. */
    private respond(body: Record<string, unknown>) {
        if (!JSON.stringify(body.system ?? "").includes("## Eidnara")) return null;
        const messages = stableSerialize(body.messages ?? []);
        if (this.pressure.kind === "served") {
            this.pressureTokens = Math.ceil(
                Buffer.byteLength(JSON.stringify(body)) / BYTES_PER_TOKEN,
            );
        }
        const share = this.previousMainMessages
            ? commonPrefixBytes(this.previousMainMessages, messages) / Math.max(1, messages.length)
            : 0;
        this.previousMainMessages = messages;
        const total = Math.max(1, this.pressureTokens);
        const read = Math.floor(total * share * 0.97);
        const input = Math.max(1, Math.floor((total - read) * 0.05));
        const write = Math.max(0, total - read - input);
        this.totalInputTokens += input + read + write;
        return {
            text: `timing assistant ${this.turnCount}`,
            usage: {
                input_tokens: input,
                output_tokens: 20,
                cache_read_input_tokens: read,
                cache_creation_input_tokens: write,
            },
        };
    }

    /** The limit the plugin measures pressure against; learned from the daemon after the first response. */
    private async limit(): Promise<number> {
        if (this.contextLimit > 0) return this.contextLimit;
        const usage = (await this.status()).usage as { context_limit_tokens?: number } | undefined;
        this.contextLimit = usage?.context_limit_tokens ?? 0;
        return this.contextLimit > 0 ? this.contextLimit : this.configuredLimit;
    }

    /** Pressure the newest response reported, as a percentage of the plugin's limit; the next pass reads it. */
    async reportedPercent(): Promise<number> {
        return (this.pressureTokens * 100) / (await this.limit());
    }

    async status(): Promise<Record<string, unknown>> {
        return this.h.host.primaryStatus(this.sessionId, this.h.env.workdir, "session.status");
    }

    /** The whole durable state, for the fields `session.status` does not project. */
    async durableStatus(): Promise<Record<string, unknown>> {
        return this.h.host.primaryStatus(this.sessionId, this.h.env.workdir, "status");
    }

    /** Removes OpenCode's messages after the first segment's last covered message: the first segment survives, while the fold boundary and every later segment's range disappear. */
    async revertAfterFirstSegment(): Promise<void> {
        const status = await this.h.host.primaryStatus(
            this.sessionId,
            this.h.env.workdir,
            "session.status",
            { include_history_segments_after_seq: -1 },
        );
        const segments = status.history_segments as
            | Array<{ sequence: number; end_message_id?: string }>
            | undefined;
        const target = segments?.find((segment) => segment.sequence === 1)?.end_message_id;
        if (!target) throw new Error("no first segment to revert after");
        const messageId = target.split("#")[0] ?? target;
        const removed = this.h.removeMessagesAfter(this.sessionId, messageId);
        if (removed === 0)
            throw new Error(`segment end ${target} matched no later OpenCode message`);
    }

    async timing(): Promise<CompactionTiming> {
        const timing = await this.h.host.compactionTiming(this.sessionId, this.h.env.workdir);
        if (!timing) throw new Error("session.status carried no summarizer timeline");
        return timing;
    }

    async rawFirings(): Promise<RawFiring[]> {
        const summarizer = (await this.status()).history_summarizer as
            | { recent_firings?: RawFiring[] }
            | undefined;
        return summarizer?.recent_firings ?? [];
    }

    /** One user turn. Scripted pressure makes this turn's response report `percent` of the plugin's limit; served pressure ignores it. The next turn's pass reads that report. */
    async turn(percent = 0): Promise<TurnRecord> {
        if (this.pressure.kind === "scripted") {
            this.pressureTokens = Math.round((percent / 100) * (await this.limit()));
        }
        const passesBefore = this.h.readRustPasses().length;
        this.turnCount += 1;
        await this.h.sendPrompt(
            this.sessionId,
            `timing turn ${this.turnCount}: ${this.h.ballast(TURN_BALLAST_TOKENS)}`,
        );
        const passes = await this.h.waitForRustPasses(passesBefore + 1);
        return { pass: passes.at(-1), timing: await this.timing() };
    }

    /** Turns at `percent` until `done` holds for the current firings, failing loudly when it never does. */
    async turnsUntil(
        percent: number,
        done: (firings: CompactionFiring[]) => boolean,
        label: string,
    ): Promise<TurnRecord> {
        for (let index = 0; index < MAX_WAIT_TURNS; index += 1) {
            const record = await this.turn(percent);
            if (done(record.timing.firings)) return record;
        }
        throw new Error(`${label} not reached in ${MAX_WAIT_TURNS} turns at ${percent}%`);
    }

    async waitForFirings(
        done: (firings: CompactionFiring[]) => boolean,
        label: string,
    ): Promise<CompactionFiring[]> {
        const deadline = Date.now() + 120_000;
        while (Date.now() < deadline) {
            const { firings } = await this.timing();
            if (done(firings)) return firings;
            await Bun.sleep(100);
        }
        throw new Error(`${label} not observed\n${this.h.host.hostLog().slice(-4_000)}`);
    }

    /** Ramps pressure and holds it until the session's first firing folds, which activates at once in any band and is excluded from tuning metrics. */
    async foldFirst(holdPercent: number): Promise<void> {
        for (const percent of [20, 35, 50]) await this.turn(percent);
        await this.turnsUntil(holdPercent, (firings) => firings.length >= 1, "first firing");
        await this.waitForFirings((firings) => published(firings[0]), "first publication");
        await this.turnsUntil(holdPercent, (firings) => activated(firings[0]), "first activation");
    }

    async report(scenario: string, runId: string): Promise<ArmReport> {
        const status = await this.status();
        const timing = await this.timing();
        const counters = timing.counters;
        const passes = this.h.readRustPasses();
        const waits = passes
            .map((pass) => pass.emergencyWaitMs)
            .filter((wait) => wait > 0)
            .sort((a, b) => a - b);
        const count = (decision: string) =>
            passes.filter((pass) => pass.decision.toUpperCase() === decision).length;
        const published = counters?.exact.published ?? null;
        const ring = (
            status.pass_trace as {
                scheduler_history?: Array<{ usage_soft_limit_tokens?: number }>;
            }
        )?.scheduler_history;
        const activation = timing.publishToActivation;
        const share = (action: (typeof CACHE_ACTIONS)[number]) =>
            timing.cacheReadShareAfter[action]?.ratio ?? null;
        const samples = (action: (typeof CACHE_ACTIONS)[number]) =>
            timing.cacheReadShareAfter[action]?.samples ?? 0;
        return {
            run_id: runId,
            scenario,
            commit: currentCommit(),
            lead: this.lead ?? "default",
            provenance: "mock",
            pressure: this.pressure.kind,
            execute_threshold_percentage: EXECUTE_THRESHOLD,
            usage_denominator: "usage_soft_limit_tokens",
            soft_limit_tokens: ring?.at(-1)?.usage_soft_limit_tokens ?? null,
            clocks: {
                emergency_wait_ms: "daemon_monotonic",
                publish_to_activation_ms: "daemon_wall_ms",
            },
            pressure_path_firings: timing.firings.filter(
                (firing) => firing.source === "pressure_path" && firing.firingSeq !== 1,
            ).length,
            evicted_firings:
                counters === undefined ? null : counters.exact.firings - timing.firings.length,
            censored_firings: activation.censored,
            passes: passes.length,
            emergency_passes: timing.emergencyPasses,
            emergency_wait_ms: { max: waits.at(-1) ?? null, ordered: waits },
            publish_to_activation_ms: {
                max: activation.maxMs ?? null,
                n: activation.n,
                ordered: activation.orderedMs,
            },
            hard_passes: count("HARD"),
            soft_passes: count("SOFT"),
            published,
            rebuilds_per_publication: published
                ? (count("HARD") + count("SOFT")) / published
                : null,
            validation_rejected: counters?.bestEffort.validationRejected ?? null,
            invalidated: counters?.bestEffort.invalidated ?? null,
            superseded_before_activation: counters?.exact.supersededBeforeActivation ?? null,
            total_input_tokens: this.totalInputTokens,
            total_input_tokens_source: "mock usage returned to the plugin, summed by the harness",
            cache_read_share_after: {
                "SOFT+": share("SOFT+"),
                SOFT: share("SOFT"),
                HARD: share("HARD"),
            },
            cache_read_samples_after: {
                "SOFT+": samples("SOFT+"),
                SOFT: samples("SOFT"),
                HARD: samples("HARD"),
            },
        };
    }

    async dispose(): Promise<void> {
        await this.h.dispose();
    }
}

export function published(firing: CompactionFiring | undefined): boolean {
    return firing?.state === "pending" || firing?.state === "activated";
}

export function activated(firing: CompactionFiring | undefined): boolean {
    return firing?.state === "activated";
}

/** One line per arm, so a sweep's output is the tuning table's raw rows. */
export function printReport(report: ArmReport): void {
    console.log(`[compaction-timing] ${JSON.stringify(report)}`);
}

/**
 * The sweep workload every arm replays unchanged: served pressure from a fixed turn sequence, a producer held for `producerTurns` turns after each firing starts, and a held producer that finishes `emergencyFinishMs` into an Emergency95 pass. Only the lead differs between arms.
 */
export const SWEEP = {
    turns: 70,
    modelContextLimit: 60_000,
    producerTurns: 12,
    emergencyFinishMs: 1_500,
    emergencyPercent: 95,
} as const;

export async function runSweepArm(lead: number | undefined, runId: string): Promise<ArmReport> {
    const session = await TimingSession.start(lead, {
        pressure: { kind: "served" },
        modelContextLimit: SWEEP.modelContextLimit,
    });
    // Re-arming only after the released run settles keeps one hold per firing.
    const settleAndRearm = async () => {
        await session.waitForFirings(
            (firings) => !firings.some((firing) => firing.state === "in_flight"),
            "released producer settles",
        );
        await session.h.host.blockNextBackendCall();
    };
    try {
        await session.h.host.blockNextBackendCall();
        let heldTurns = 0;
        for (let turn = 0; turn < SWEEP.turns; turn += 1) {
            const emergencyNext = (await session.reportedPercent()) >= SWEEP.emergencyPercent;
            // An Emergency95 pass waits on a live run or fires inline; either way the held producer finishes into the wait. The release retries because an inline firing's call can start after the delay.
            let settled = false;
            const finish = emergencyNext
                ? (async () => {
                      await Bun.sleep(SWEEP.emergencyFinishMs);
                      while (!settled) {
                          if (await session.h.host.releaseBlockedBackendCall()) return true;
                          await Bun.sleep(100);
                      }
                      return false;
                  })()
                : Promise.resolve(false);
            let record: TurnRecord;
            try {
                record = await session.turn();
            } finally {
                settled = true;
            }
            if (await finish.catch(() => false)) {
                heldTurns = 0;
                await settleAndRearm();
                continue;
            }
            if (!record.timing.firings.some((firing) => firing.state === "in_flight")) {
                heldTurns = 0;
                continue;
            }
            heldTurns += 1;
            if (heldTurns >= SWEEP.producerTurns) {
                await session.h.host.releaseBlockedBackendCall();
                heldTurns = 0;
                await settleAndRearm();
            }
        }
        return await session.report(`rust-compaction-timing:sweep`, runId);
    } finally {
        await session.h.host.releaseBlockedBackendCall().catch(() => false);
        await session.dispose();
    }
}
