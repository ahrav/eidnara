import { describe, expect, it } from "bun:test";
import { formatCompactionTimingLines, summarizeCompactionTiming } from "./compaction-timing";

const CLOCK = "daemon_wall_ms";

function firing(seq: number, stamps: Record<string, number>, outcome?: unknown, clock = CLOCK) {
    return { firing_seq: seq, source: "pressure_path", clock, ...stamps, outcome };
}

const published = (sequence: number) => ({ kind: "published", sequence });

function counters(values: Partial<Record<string, number>> = {}) {
    return {
        firings: 0,
        published: 0,
        superseded_before_activation: 0,
        validation_rejected: 0,
        invalidated: 0,
        connect_failed: 0,
        ...values,
    };
}

function ringEntry(
    action: string,
    reason: string | undefined,
    cache?: [number, number],
    decision = "Defer",
) {
    return {
        timestamp_ms: 1,
        scheduler_decision: decision,
        drain_latch_active: false,
        action,
        ...(reason ? { materialize_reason: reason } : {}),
        ...(cache
            ? { prev_response_cache: { cache_read_tokens: cache[0], cache_write_tokens: cache[1] } }
            : {}),
    };
}

describe("summarizeCompactionTiming", () => {
    it("returns nothing for a status without the timeline, so older payloads render unchanged", () => {
        const status = { history_summarizer: { consecutive_publish_failures: 0 } };
        expect(summarizeCompactionTiming("old", status)).toBeUndefined();
        expect(formatCompactionTimingLines(summarizeCompactionTiming("old", status))).toEqual([]);
        expect(summarizeCompactionTiming("old", undefined)).toBeUndefined();
    });

    it("derives both durations for an activated firing and marks a pending one unactivated", () => {
        const timing = summarizeCompactionTiming("durations", {
            history_summarizer: {
                recent_firings: [
                    firing(
                        1,
                        {
                            eligible_at_ms: 0,
                            fired_at_ms: 0,
                            published_at_ms: 10,
                            activated_at_ms: 10,
                        },
                        published(1),
                    ),
                    firing(
                        2,
                        {
                            eligible_at_ms: 100,
                            fired_at_ms: 1_100,
                            published_at_ms: 4_100,
                            activated_at_ms: 9_100,
                        },
                        published(2),
                    ),
                    firing(
                        3,
                        { eligible_at_ms: 10_000, fired_at_ms: 10_000, published_at_ms: 12_000 },
                        published(3),
                    ),
                ],
                counters: counters({ firings: 3, published: 3 }),
            },
        });
        const [, activated, pending] = timing?.firings ?? [];
        expect(activated).toMatchObject({
            state: "activated",
            executionDelayMs: 1_000,
            fireToPublishMs: 3_000,
            publishToActivationMs: 5_000,
        });
        expect(pending).toMatchObject({ state: "pending", publishToActivationMs: undefined });
        // The first publication folds immediately and is left out; the pending one is censored.
        expect(timing?.publishToActivation).toEqual({
            maxMs: 5_000,
            n: 1,
            orderedMs: [5_000],
            censored: 1,
        });
        const lines = formatCompactionTimingLines(timing);
        expect(lines[0]).toBe("### Compaction Timing");
        expect(lines).toContain("- Last summary #3 (pressure path): published, not yet activated");
        expect(lines).toContain("- Waited to start 0.0s, ran 2.0s, sat unactivated still");
        expect(lines).toContain("- Publish to activation: max 5.0s over 1 (5.0s), 1 pending");
    });

    it("renders a missing, inverted, or differently clocked stamp as unknown", () => {
        const timing = summarizeCompactionTiming("unknown", {
            history_summarizer: {
                recent_firings: [
                    firing(
                        4,
                        { fired_at_ms: 50 },
                        { kind: "abandoned", class: "validation_rejected" },
                    ),
                    firing(
                        5,
                        { eligible_at_ms: 90, fired_at_ms: 80, published_at_ms: 100 },
                        published(5),
                    ),
                    firing(
                        6,
                        { eligible_at_ms: 1, fired_at_ms: 2, published_at_ms: 3 },
                        published(6),
                        "other_clock",
                    ),
                ],
            },
        });
        const [abandoned, inverted, foreign] = timing?.firings ?? [];
        expect(abandoned).toMatchObject({
            state: "abandoned",
            abandonClass: "validation_rejected",
        });
        expect(abandoned?.executionDelayMs).toBeUndefined();
        expect(inverted?.executionDelayMs).toBeUndefined();
        expect(inverted?.fireToPublishMs).toBe(20);
        expect(foreign?.fireToPublishMs).toBeUndefined();
        expect(formatCompactionTimingLines(timing)).toContain(
            "- Waited to start unknown, ran unknown, sat unactivated still",
        );
    });

    it("labels exact and best-effort counts and marks a decrease as a reset boundary", () => {
        const read = (values: Partial<Record<string, number>>) =>
            summarizeCompactionTiming("reset", {
                history_summarizer: { counters: counters(values) },
            });
        expect(read({ firings: 5, published: 4, validation_rejected: 1 })?.counters).toEqual({
            exact: { firings: 5, published: 4, supersededBeforeActivation: 0 },
            bestEffort: { validationRejected: 1, invalidated: 0, connectFailed: 0 },
            resetSincePreviousRead: false,
        });
        const reset = read({ firings: 1 });
        expect(reset?.counters?.resetSincePreviousRead).toBe(true);
        expect(formatCompactionTimingLines(reset)).toContain(
            "- Best effort: validation rejected 0, invalidated 0, connect failed 0 (counts reset since the last read)",
        );
    });

    it("counts passes by reason and scores each response against the pass before it, once per request", () => {
        const timing = summarizeCompactionTiming("ring", {
            history_summarizer: { recent_firings: [] },
            pass_trace: {
                scheduler_history: [
                    ringEntry("HARD", "first_render"),
                    ringEntry("SOFT+", undefined, [0, 1_000]),
                    ringEntry("SOFT+", undefined, [900, 100], "Emergency95"),
                    // The emergency rerun repeats the request's sample and is not a second response.
                    ringEntry("SOFT", "coverage_fold", [900, 100], "Execute"),
                    ringEntry("SOFT+", undefined, [0, 0]),
                    ringEntry("SOFT+", undefined, [800, 200]),
                ],
            },
        });
        expect(timing?.passesByReason).toEqual({
            "HARD first_render": 1,
            "SOFT+ -": 4,
            "SOFT coverage_fold": 1,
        });
        // HARD → write-heavy sample; SOFT+ → read-heavy; the zero sample after SOFT carries no ratio.
        expect(timing?.cacheReadShareAfter).toEqual({
            HARD: { ratio: 0, samples: 1 },
            "SOFT+": { ratio: (900 + 800) / 2_000, samples: 2 },
        });
        expect(formatCompactionTimingLines(timing)).toContain(
            "- Cache read share after: HARD 0% (1), SOFT+ 85% (2)",
        );
    });
});
