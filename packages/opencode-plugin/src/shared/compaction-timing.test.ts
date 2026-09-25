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

/** `at` is the request's pass clock, which every rerun of that request repeats. */
function ringEntry(
    at: number,
    action: string,
    reason: string | undefined,
    cache?: [number, number],
    decision = "Defer",
) {
    return {
        timestamp_ms: at,
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
        expect(summarizeCompactionTiming("old", "/root", status)).toBeUndefined();
        expect(
            formatCompactionTimingLines(summarizeCompactionTiming("old", "/root", status)),
        ).toEqual([]);
        expect(summarizeCompactionTiming("old", "/root", undefined)).toBeUndefined();
    });

    it("derives both durations for an activated firing and marks a pending one unactivated", () => {
        const timing = summarizeCompactionTiming("durations", "/root", {
            history_summarizer: {
                recent_firings: [
                    {
                        ...firing(
                            1,
                            {
                                eligible_at_ms: 0,
                                fired_at_ms: 0,
                                published_at_ms: 10,
                                activated_at_ms: 10,
                            },
                            published(1),
                        ),
                        activated_by_first_fold: true,
                    },
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

    it("excludes only a first fold, even when the counters match the window", () => {
        const only = (entry: unknown, boundaryPresent?: boolean) =>
            summarizeCompactionTiming("first-fold", "/root", {
                ...(boundaryPresent === undefined ? {} : { boundary_present: boundaryPresent }),
                history_summarizer: {
                    recent_firings: [entry],
                    counters: counters({ firings: 1, published: 1 }),
                },
            })?.publishToActivation;
        expect(
            only(
                firing(
                    1,
                    { fired_at_ms: 0, published_at_ms: 10, activated_at_ms: 4_010 },
                    published(9),
                ),
            ),
        ).toEqual({ maxMs: 4_000, n: 1, orderedMs: [4_000], censored: 0 });
        expect(
            only({
                ...firing(1, { published_at_ms: 10, activated_at_ms: 20 }, published(1)),
                activated_by_first_fold: true,
            }),
        ).toEqual({ n: 0, orderedMs: [], censored: 0 });
        const pending = firing(1, { published_at_ms: 10 }, published(1));
        expect(only(pending, false)?.censored).toBe(0);
        expect(only(pending, true)?.censored).toBe(1);
        expect(only(pending)?.censored).toBe(1);
    });

    it("reads a publication a revert removed as superseded, never pending or censored", () => {
        const timing = summarizeCompactionTiming("superseded", "/root", {
            history_summarizer: {
                recent_firings: [
                    firing(
                        1,
                        { fired_at_ms: 0, published_at_ms: 5, activated_at_ms: 5 },
                        published(1),
                    ),
                    firing(
                        2,
                        { fired_at_ms: 10, published_at_ms: 20 },
                        { kind: "published", sequence: null },
                    ),
                ],
                counters: counters({ firings: 2, published: 2, superseded_before_activation: 1 }),
            },
        });
        expect(timing?.firings[1]?.state).toBe("superseded");
        expect(timing?.publishToActivation.censored).toBe(0);
        expect(formatCompactionTimingLines(timing)).toContain(
            "- Last summary #2 (pressure path): published, superseded before activation",
        );
    });

    it("renders a missing, inverted, or differently clocked stamp as unknown", () => {
        const timing = summarizeCompactionTiming("unknown", "/root", {
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
            summarizeCompactionTiming("reset", "/root", {
                history_summarizer: { counters: counters(values) },
            });
        expect(read({ firings: 5, published: 4, validation_rejected: 1 })?.counters).toEqual({
            exact: { firings: 5, published: 4, supersededBeforeActivation: 0 },
            bestEffort: { validationRejected: 1, invalidated: 0, connectFailed: 0 },
            resetSincePreviousRead: false,
        });
        const reset = read({ firings: 1 });
        expect(reset?.counters?.resetSincePreviousRead).toBe(true);
        expect(formatCompactionTimingLines(reset)).toContain("- Counts reset since the last read");
    });

    it("keeps counter history per (session, project root), the identity the daemon keys status by", () => {
        const read = (root: string, firings: number) =>
            summarizeCompactionTiming("shared-id", root, {
                history_summarizer: { counters: counters({ firings }) },
            })?.counters?.resetSincePreviousRead;
        expect(read("/a", 10)).toBe(false);
        expect(read("/b", 1)).toBe(false);
        expect(read("/a", 11)).toBe(false);
        expect(read("/a", 2)).toBe(true);
    });

    it("counts passes by reason and scores each response against the pass before it, once per request", () => {
        const timing = summarizeCompactionTiming("ring", "/root", {
            history_summarizer: { recent_firings: [] },
            pass_trace: {
                scheduler_history: [
                    ringEntry(10, "HARD", "first_render"),
                    ringEntry(20, "SOFT+", undefined, [0, 1_000]),
                    ringEntry(30, "SOFT+", undefined, [900, 100], "Emergency95"),
                    // The emergency rerun runs inside the same request and is not a second response.
                    ringEntry(30, "SOFT", "coverage_fold", [900, 100], "Execute"),
                    ringEntry(40, "SOFT+", undefined, [0, 0]),
                    ringEntry(50, "SOFT+", undefined, [800, 200]),
                ],
            },
        });
        expect(timing?.passesByReason).toEqual({
            "HARD first_render": 1,
            "SOFT+ -": 3,
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

    it("groups a request by its pass clock, never by an equal or missing cache sample", () => {
        const timing = summarizeCompactionTiming("request-identity", "/root", {
            history_summarizer: { recent_firings: [] },
            pass_trace: {
                scheduler_history: [
                    // An Emergency95 request that settled without a rerun, then a distinct request whose
                    // provider reported the same zero cache counts.
                    ringEntry(10, "SOFT", "m1_delta", [0, 0], "Emergency95"),
                    ringEntry(20, "HARD", "hard_trigger", [0, 0]),
                    // An Emergency95 request whose provider reported no cache counts, and its rerun.
                    ringEntry(30, "SOFT+", undefined, undefined, "Emergency95"),
                    ringEntry(30, "SOFT", "coverage_fold", undefined, "Execute"),
                    ringEntry(40, "SOFT+", undefined, [600, 400]),
                ],
            },
        });
        expect(timing?.passesByReason).toEqual({
            "SOFT m1_delta": 1,
            "HARD hard_trigger": 1,
            "SOFT coverage_fold": 1,
            "SOFT+ -": 1,
        });
        // Only the rerun's served action is scored against the next request's sample.
        expect(timing?.cacheReadShareAfter).toEqual({
            SOFT: { ratio: 0.6, samples: 1 },
        });
    });
});
