/**
 * Reads a `session.status` payload's summarizer timeline, counters, and pass ring into the compaction-timing view both `/eidnara-status` readers render. Everything here is derived at read: the daemon stores stamps and counts, never durations, ratios, or tallies.
 */

import { BoundedSessionMap } from "./bounded-session-map";

const TIMELINE_CLOCK = "daemon_wall_ms";

export interface CompactionFiring {
    firingSeq: number;
    source: string;
    /** `activated` once a pass rendered it, `pending` while published and unrendered, otherwise the outcome kind. */
    state: "activated" | "pending" | "abandoned" | "in_flight" | "reattach_connect_failed";
    abandonClass?: string;
    /** Eligibility to fire: time a run could not start. */
    executionDelayMs?: number;
    /** Fire to publish: the producer's run. */
    fireToPublishMs?: number;
    /** Publish to activation: time the summary waited unrendered. */
    publishToActivationMs?: number;
}

export interface CompactionCounters {
    /** Committed with the transition they count. */
    exact: { firings: number; published: number; supersededBeforeActivation: number };
    /** Written after the failure; a crash in between loses one. */
    bestEffort: { validationRejected: number; invalidated: number; connectFailed: number };
    /** A count fell since the previous read: the session was reset, so no delta across this read is meaningful. */
    resetSincePreviousRead: boolean;
}

export interface CacheReadShare {
    /** Pooled `read / (read + write)` over the responses that followed passes with this action. */
    ratio: number;
    samples: number;
}

export interface CompactionTiming {
    firings: CompactionFiring[];
    counters?: CompactionCounters;
    /** Publish-to-activation over activated firings, excluding the session's first firing, which folds immediately. */
    publishToActivation: { maxMs?: number; n: number; orderedMs: number[]; censored: number };
    /** Pass counts by `ACTION reason` over the ring. */
    passesByReason: Record<string, number>;
    cacheReadShareAfter: Record<string, CacheReadShare>;
}

type Json = Record<string, unknown>;

const previousCounters = new BoundedSessionMap<Record<string, number>>(64);

function record(value: unknown): Json | undefined {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Json)
        : undefined;
}

function stamp(value: unknown): number | undefined {
    return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function count(value: Json, key: string): number {
    const found = value[key];
    return typeof found === "number" && Number.isFinite(found) ? found : 0;
}

/** Unknown unless both stamps exist, come from the timeline clock, and run forward. */
function duration(from: unknown, to: unknown, clock: unknown): number | undefined {
    const start = stamp(from);
    const end = stamp(to);
    if (clock !== TIMELINE_CLOCK || start === undefined || end === undefined || end < start) {
        return undefined;
    }
    return end - start;
}

function readFiring(entry: Json): CompactionFiring {
    const outcome = record(entry.outcome);
    const clock = entry.clock;
    const state: CompactionFiring["state"] =
        outcome?.kind === "published"
            ? stamp(entry.activated_at_ms) === undefined
                ? "pending"
                : "activated"
            : outcome?.kind === "abandoned"
              ? "abandoned"
              : outcome?.kind === "reattach_connect_failed"
                ? "reattach_connect_failed"
                : "in_flight";
    return {
        firingSeq: count(entry, "firing_seq"),
        source: typeof entry.source === "string" ? entry.source : "unknown",
        state,
        ...(typeof outcome?.class === "string" ? { abandonClass: outcome.class } : {}),
        executionDelayMs: duration(entry.eligible_at_ms, entry.fired_at_ms, clock),
        fireToPublishMs: duration(entry.fired_at_ms, entry.published_at_ms, clock),
        publishToActivationMs: duration(entry.published_at_ms, entry.activated_at_ms, clock),
    };
}

function readCounters(sessionId: string, value: Json): CompactionCounters {
    const current: Record<string, number> = {};
    for (const key of [
        "firings",
        "published",
        "superseded_before_activation",
        "validation_rejected",
        "invalidated",
        "connect_failed",
    ]) {
        current[key] = count(value, key);
    }
    const previous = previousCounters.get(sessionId);
    previousCounters.set(sessionId, current);
    return {
        exact: {
            firings: current.firings ?? 0,
            published: current.published ?? 0,
            supersededBeforeActivation: current.superseded_before_activation ?? 0,
        },
        bestEffort: {
            validationRejected: current.validation_rejected ?? 0,
            invalidated: current.invalidated ?? 0,
            connectFailed: current.connect_failed ?? 0,
        },
        resetSincePreviousRead:
            previous !== undefined &&
            Object.entries(current).some(([key, value]) => value < (previous[key] ?? 0)),
    };
}

interface RingEntry {
    decision: string;
    action?: string;
    reason?: string;
    cache?: { read: number; write: number };
}

function readRing(value: unknown): RingEntry[] {
    if (!Array.isArray(value)) return [];
    return value.flatMap((raw) => {
        const entry = record(raw);
        if (!entry) return [];
        const cache = record(entry.prev_response_cache);
        const read = cache && stamp(cache.cache_read_tokens);
        const write = cache && stamp(cache.cache_write_tokens);
        return [
            {
                decision:
                    typeof entry.scheduler_decision === "string" ? entry.scheduler_decision : "",
                ...(typeof entry.action === "string" ? { action: entry.action } : {}),
                ...(typeof entry.materialize_reason === "string"
                    ? { reason: entry.materialize_reason }
                    : {}),
                ...(read !== undefined && write !== undefined ? { cache: { read, write } } : {}),
            },
        ];
    });
}

/** One plugin request per group: an Emergency95 pass reruns inside the same request, appending an entry with the same cache sample. */
function groupByRequest(ring: RingEntry[]): RingEntry[][] {
    const requests: RingEntry[][] = [];
    for (const entry of ring) {
        const last = requests.at(-1);
        const previous = last?.at(-1);
        const rerun =
            previous?.decision === "Emergency95" &&
            previous.cache !== undefined &&
            entry.cache?.read === previous.cache.read &&
            entry.cache?.write === previous.cache.write;
        if (last && rerun) last.push(entry);
        else requests.push([entry]);
    }
    return requests;
}

/** The cache sample on a request's first entry describes the response to the previous request, so it scores that request's served action. */
function cacheReadShareAfter(ring: RingEntry[]): Record<string, CacheReadShare> {
    const pooled: Record<string, { read: number; total: number; samples: number }> = {};
    const requests = groupByRequest(ring);
    for (let index = 0; index + 1 < requests.length; index += 1) {
        const action = requests[index]?.at(-1)?.action;
        const sample = requests[index + 1]?.[0]?.cache;
        if (!action || !sample || sample.read + sample.write === 0) continue;
        const slot = pooled[action] ?? { read: 0, total: 0, samples: 0 };
        slot.read += sample.read;
        slot.total += sample.read + sample.write;
        slot.samples += 1;
        pooled[action] = slot;
    }
    return Object.fromEntries(
        Object.entries(pooled).map(([action, slot]) => [
            action,
            { ratio: slot.read / slot.total, samples: slot.samples },
        ]),
    );
}

/** `undefined` when the payload carries no summarizer timeline, so an older daemon's status renders unchanged. */
export function summarizeCompactionTiming(
    sessionId: string,
    status: unknown,
): CompactionTiming | undefined {
    const summarizer = record(record(status)?.history_summarizer);
    const rawFirings = summarizer?.recent_firings;
    const rawCounters = record(summarizer?.counters);
    if (!Array.isArray(rawFirings) && !rawCounters) return undefined;
    const entries = (Array.isArray(rawFirings) ? rawFirings : []).flatMap((raw) => {
        const entry = record(raw);
        return entry ? [entry] : [];
    });
    const firings = entries.map(readFiring);
    const counters = rawCounters ? readCounters(sessionId, rawCounters) : undefined;
    // The session's first publication folds immediately; it is excluded only while the window still holds it.
    const published = firings.filter(
        (firing) => firing.state === "activated" || firing.state === "pending",
    );
    const first =
        counters !== undefined && counters.exact.published === published.length
            ? published[0]
            : undefined;
    const measured = firings.filter((firing) => firing !== first);
    const orderedMs = measured
        .flatMap((firing) =>
            firing.publishToActivationMs === undefined ? [] : [firing.publishToActivationMs],
        )
        .sort((a, b) => a - b);
    const ring = readRing(record(record(status)?.pass_trace)?.scheduler_history);
    const passesByReason: Record<string, number> = {};
    for (const entry of ring) {
        if (!entry.action) continue;
        const key = `${entry.action} ${entry.reason ?? "-"}`;
        passesByReason[key] = (passesByReason[key] ?? 0) + 1;
    }
    return {
        firings,
        ...(counters ? { counters } : {}),
        publishToActivation: {
            ...(orderedMs.length > 0 ? { maxMs: orderedMs.at(-1) } : {}),
            n: orderedMs.length,
            orderedMs,
            censored: measured.filter((firing) => firing.state === "pending").length,
        },
        passesByReason,
        cacheReadShareAfter: cacheReadShareAfter(ring),
    };
}

function ms(value: number | undefined): string {
    return value === undefined ? "unknown" : `${(value / 1000).toFixed(1)}s`;
}

/** The lines both readers show; empty when nothing was recorded. */
export function formatCompactionTimingLines(timing: CompactionTiming | undefined): string[] {
    if (!timing) return [];
    const lines: string[] = [];
    const last = timing.firings.at(-1);
    if (last) {
        const state =
            last.state === "abandoned" && last.abandonClass
                ? `abandoned (${last.abandonClass})`
                : last.state === "pending"
                  ? "published, not yet activated"
                  : last.state.replaceAll("_", " ");
        lines.push(
            `- Last summary #${last.firingSeq} (${last.source.replaceAll("_", " ")}): ${state}`,
            `- Waited to start ${ms(last.executionDelayMs)}, ran ${ms(last.fireToPublishMs)}, sat unactivated ${last.state === "pending" ? "still" : ms(last.publishToActivationMs)}`,
        );
    }
    const activation = timing.publishToActivation;
    if (activation.n > 0 || activation.censored > 0) {
        lines.push(
            `- Publish to activation: max ${ms(activation.maxMs)} over ${activation.n} (${activation.orderedMs.map((value) => ms(value)).join(", ") || "none"}), ${activation.censored} pending`,
        );
    }
    const counters = timing.counters;
    if (counters) {
        const { exact, bestEffort } = counters;
        lines.push(
            `- Firings ${exact.firings}, published ${exact.published}, superseded before activation ${exact.supersededBeforeActivation}`,
            `- Best effort: validation rejected ${bestEffort.validationRejected}, invalidated ${bestEffort.invalidated}, connect failed ${bestEffort.connectFailed}${counters.resetSincePreviousRead ? " (counts reset since the last read)" : ""}`,
        );
    }
    const reasons = Object.entries(timing.passesByReason).sort(([a], [b]) => a.localeCompare(b));
    if (reasons.length > 0) {
        lines.push(`- Pass reasons: ${reasons.map(([key, n]) => `${key} ×${n}`).join(", ")}`);
    }
    const shares = Object.entries(timing.cacheReadShareAfter).sort(([a], [b]) =>
        a.localeCompare(b),
    );
    if (shares.length > 0) {
        lines.push(
            `- Cache read share after: ${shares.map(([action, share]) => `${action} ${(share.ratio * 100).toFixed(0)}% (${share.samples})`).join(", ")}`,
        );
    }
    return lines.length > 0 ? ["### Compaction Timing", ...lines] : [];
}
