/**
 * Drives one fixed-lead arm of the compaction-timing scenario through the mock provider and the direct-host fixture, and reads every measurement back: firing timelines and counters from `session.status` through the shared status helper, served action, reason, and emergency wait from the plugin's `rust pass:` lines, and provider input from what the mock reported to the plugin.
 *
 * Pressure is scripted per turn as a percentage of the plugin's context limit, independent of the transcript's real size, so arms differ only in the lead. The mock splits each response's input into cache read and write by how much of the previous main request's message prefix the new request repeats, so cache evidence follows what the daemon served rather than being scripted per action.
 */

import { execFileSync } from "node:child_process";
import {
    type CompactionFiring,
    type CompactionTiming,
    formatCompactionTimingLines,
} from "@eidnara/opencode/shared/compaction-timing";
import { type RustPassLine, RustTestHarness, stableSerialize } from "./rust-harness";

export const EXECUTE_THRESHOLD = 65;
export const PREPARE_LEAD_ENV = "EIDNARA_HISTORY_SUMMARIZER_PREPARE_LEAD";
const MODEL_CONTEXT_LIMIT = 100_000;
const TURN_BALLAST_TOKENS = 900;
const MAX_WAIT_TURNS = 40;

/** The raw firing fields the shared helper does not project. */
export interface RawFiring {
    usage?: { usage_percentage?: number };
    outcome?: { sequence?: number | null };
}

export interface TurnRecord {
    /** Pressure this turn's response reported; the next turn's pass reads it. */
    percent: number;
    pass: RustPassLine | undefined;
    timing: CompactionTiming;
}

export interface ArmReport {
    run_id: string;
    scenario: string;
    commit: string;
    lead: number | "default";
    provenance: "mock";
    execute_threshold_percentage: number;
    usage_denominator: "usage_soft_limit_tokens";
    /** `null` when the newest ring entry carries no soft limit. */
    soft_limit_tokens: number | null;
    clock: "daemon_wall_ms";
    pressure_path_firings: number;
    censored_firings: number;
    passes: number;
    emergency_passes: number;
    emergency_wait_ms: { max: number | null; ordered: number[] };
    publish_to_activation_ms: { max: number | null; n: number; ordered: number[] };
    hard_passes: number;
    soft_passes: number;
    published: number;
    rebuilds_per_publication: number | null;
    validation_rejected: number;
    invalidated: number;
    superseded_before_activation: number;
    total_input_tokens: number;
    total_input_tokens_source: "plugin-reported provider usage";
    cache_read_share_after: CompactionTiming["cacheReadShareAfter"];
    status_lines: string[];
}

function commonPrefixBytes(previous: string, next: string): number {
    const limit = Math.min(previous.length, next.length);
    let index = 0;
    while (index < limit && previous.charCodeAt(index) === next.charCodeAt(index)) index += 1;
    return index;
}

function currentCommit(): string {
    try {
        return execFileSync("git", ["rev-parse", "--short", "HEAD"], { encoding: "utf8" }).trim();
    } catch {
        return "unknown";
    }
}

export class TimingSession {
    readonly h: RustTestHarness;
    readonly sessionId: string;
    readonly lead: number | undefined;
    readonly turns: TurnRecord[] = [];
    private pressureTokens = 0;
    private previousMainMessages = "";
    private softLimit = 0;
    totalInputTokens = 0;

    private constructor(h: RustTestHarness, sessionId: string, lead: number | undefined) {
        this.h = h;
        this.sessionId = sessionId;
        this.lead = lead;
    }

    /** `lead` undefined runs the daemon's built-in default, the baseline arm. */
    static async start(lead: number | undefined): Promise<TimingSession> {
        const h = await RustTestHarness.create({
            modelContextLimit: MODEL_CONTEXT_LIMIT,
            eidnaraConfig: {
                execute_threshold_percentage: EXECUTE_THRESHOLD,
                protected_tags: 1,
                history_summarizer: { model: "fixture/deterministic" },
            },
            daemonEnv: lead === undefined ? {} : { [PREPARE_LEAD_ENV]: String(lead) },
        });
        await h.host.backendSuccess();
        const session = new TimingSession(h, await h.createSession(), lead);
        h.mock.addMatcher((body) => session.respond(body));
        return session;
    }

    /** Main requests carry the Eidnara system block; everything else falls through to the default. */
    private respond(body: Record<string, unknown>) {
        if (!JSON.stringify(body.system ?? "").includes("## Eidnara")) return null;
        const messages = stableSerialize(body.messages ?? []);
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
            text: `timing assistant ${this.turns.length}`,
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
        if (this.softLimit > 0) return this.softLimit;
        const status = await this.status();
        const usage = status.usage as { context_limit_tokens?: number } | undefined;
        this.softLimit = usage?.context_limit_tokens ?? 0;
        return this.softLimit > 0 ? this.softLimit : MODEL_CONTEXT_LIMIT;
    }

    async status(): Promise<Record<string, unknown>> {
        return this.h.host.primaryStatus(this.sessionId, this.h.env.workdir, "session.status");
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

    /** One user turn whose provider response reports `percent` of the plugin's limit; the transform pass that serves the next request reads it. */
    async turn(percent: number): Promise<TurnRecord> {
        this.pressureTokens = Math.round((percent / 100) * (await this.limit()));
        const passesBefore = this.h.readRustPasses().length;
        await this.h.sendPrompt(
            this.sessionId,
            `timing turn ${this.turns.length} at ${percent}%: ${this.h.ballast(TURN_BALLAST_TOKENS)}`,
        );
        const passes = await this.h.waitForRustPasses(passesBefore + 1);
        const record: TurnRecord = {
            percent,
            pass: passes.at(-1),
            timing: await this.timing(),
        };
        this.turns.push(record);
        return record;
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
        if (removed === 0) throw new Error(`segment end ${target} matched no OpenCode message`);
    }

    /** Ramps pressure and holds it below the execute threshold until the session's first firing folds, which activates at once in any band and is excluded from tuning metrics. */
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
        const published = counters?.exact.published ?? 0;
        const ring = (
            status.pass_trace as { scheduler_history?: Array<{ usage_soft_limit_tokens?: number }> }
        )?.scheduler_history;
        const activation = timing.publishToActivation;
        return {
            run_id: runId,
            scenario,
            commit: currentCommit(),
            lead: this.lead ?? "default",
            provenance: "mock",
            execute_threshold_percentage: EXECUTE_THRESHOLD,
            usage_denominator: "usage_soft_limit_tokens",
            soft_limit_tokens: ring?.at(-1)?.usage_soft_limit_tokens ?? null,
            clock: "daemon_wall_ms",
            pressure_path_firings: Math.max(
                0,
                timing.firings.filter((firing) => firing.source === "pressure_path").length - 1,
            ),
            censored_firings: activation.censored,
            passes: passes.length,
            emergency_passes: waits.length,
            emergency_wait_ms: { max: waits.at(-1) ?? null, ordered: waits },
            publish_to_activation_ms: {
                max: activation.maxMs ?? null,
                n: activation.n,
                ordered: activation.orderedMs,
            },
            hard_passes: count("HARD"),
            soft_passes: count("SOFT"),
            published,
            rebuilds_per_publication:
                published > 0 ? (count("HARD") + count("SOFT")) / published : null,
            validation_rejected: counters?.bestEffort.validationRejected ?? 0,
            invalidated: counters?.bestEffort.invalidated ?? 0,
            superseded_before_activation: counters?.exact.supersededBeforeActivation ?? 0,
            total_input_tokens: this.totalInputTokens,
            total_input_tokens_source: "plugin-reported provider usage",
            cache_read_share_after: timing.cacheReadShareAfter,
            status_lines: formatCompactionTimingLines(timing),
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
