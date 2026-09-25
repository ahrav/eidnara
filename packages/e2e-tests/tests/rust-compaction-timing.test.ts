/**
 * The compaction-timing scenario: fixed-lead arms through fire, controlled producer delay, pending wait, and activation, plus emergency growth and the two revert cases. Every durable claim is read back from `session.status`; served actions and emergency waits come from the plugin's `rust pass:` lines. Each case asserts the branch it names was reached, so a run cannot pass without exercising it.
 */

import { describe, expect, it } from "bun:test";
import {
    activated,
    EXECUTE_THRESHOLD,
    printReport,
    published,
    TimingSession,
} from "../src/compaction-timing-scenario";
import {
    FOLD_SKIP_REASON,
    foldInfraEnabled,
    printSkip,
    rustPrereqs,
} from "../src/rust-scenario-support";

const SCENARIO = "rust-compaction-timing";
const CASE_TIMEOUT_MS = 600_000;
/** Below the execute threshold and at or above the proactive percentage of both arms. */
const LEAD_2_HOLD = 64;
const LEAD_10_HOLD = 56;
const EXECUTE = EXECUTE_THRESHOLD + 1;
const EMERGENCY = 96;
/** Below the proactive percentage, so turns there grow the eligible head without firing. */
const QUIET = 50;
const QUIET_TURNS = 8;
const PRODUCER_DELAY_TURNS = 2;

const runId = `${SCENARIO}-${process.pid}`;
const active = rustPrereqs.ok && foldInfraEnabled();

async function withSession(
    lead: number | undefined,
    body: (session: TimingSession) => Promise<void>,
): Promise<void> {
    const session = await TimingSession.start(lead);
    try {
        await body(session);
    } finally {
        await session.dispose();
    }
}

/** Fire with the producer held, publish after a controlled delay, wait pending below the threshold, activate on the first Execute pass. Returns the measured firing's usage. */
async function leadArm(session: TimingSession, hold: number, label: string): Promise<number> {
    await session.foldFirst(hold);
    await session.h.host.blockNextBackendCall();
    await session.turnsUntil(hold, (firings) => firings.length >= 2, "measured firing");
    const usage = (await session.rawFirings())[1]?.usage?.usage_percentage;
    if (usage === undefined) throw new Error("measured firing recorded no usage");

    for (let turn = 0; turn < PRODUCER_DELAY_TURNS; turn += 1) {
        const held = await session.turn(hold);
        expect(published(held.timing.firings[1])).toBe(false);
    }
    expect(await session.h.host.releaseBlockedBackendCall()).toBe(true);
    await session.waitForFirings((firings) => published(firings[1]), "measured publication");

    const waiting = await session.turn(hold);
    expect(waiting.pass?.decision).toBe("SOFT+");
    expect(waiting.timing.firings[1]?.state).toBe("pending");

    // The pass that serves this turn still reads the held pressure; the next pass is the first at Execute.
    const reported = await session.turn(EXECUTE);
    expect(activated(reported.timing.firings[1])).toBe(false);
    const execute = await session.turn(EXECUTE);
    expect(activated(execute.timing.firings[1])).toBe(true);
    expect(execute.pass?.decision).not.toBe("SOFT+");
    // Requests carried the previous response's cache fields, so the served actions pair with samples.
    expect(execute.timing.cacheReadShareAfter["SOFT+"]?.samples ?? 0).toBeGreaterThan(0);
    expect(execute.timing.cacheReadShareAfter.HARD?.samples ?? 0).toBeGreaterThan(0);

    printReport(await session.report(`${SCENARIO}:${label}`, runId));
    return usage;
}

describe.skipIf(!rustPrereqs.ok)("rust compaction timing", () => {
    it.skipIf(active)("is gated on the fold infrastructure", () => {
        printSkip(SCENARIO, FOLD_SKIP_REASON);
        expect(foldInfraEnabled()).toBe(false);
    });

    it.skipIf(!active)(
        "moves the fire earlier with the lead and keeps activation at the execute threshold",
        async () => {
            let lead2 = 0;
            await withSession(2, async (session) => {
                lead2 = await leadArm(session, LEAD_2_HOLD, "lead-2");
            });
            expect(lead2).toBeGreaterThanOrEqual(EXECUTE_THRESHOLD - 2);
            expect(lead2).toBeLessThan(EXECUTE_THRESHOLD);

            let lead10 = 0;
            await withSession(10, async (session) => {
                lead10 = await leadArm(session, LEAD_10_HOLD, "lead-10");
            });
            expect(lead10).toBeGreaterThanOrEqual(EXECUTE_THRESHOLD - 10);
            expect(lead10).toBeLessThan(lead2);
        },
        CASE_TIMEOUT_MS,
    );

    it.skipIf(!active)(
        "waits at emergency pressure on one inline firing and counts its rerun once",
        async () => {
            await withSession(2, async (session) => {
                await session.foldFirst(LEAD_2_HOLD);
                for (let turn = 0; turn < QUIET_TURNS; turn += 1) {
                    const quiet = await session.turn(QUIET);
                    expect(quiet.timing.firings).toHaveLength(1);
                }
                // The emergency pass fires inline and waits on the held producer until the release.
                await session.h.host.blockNextBackendCall();
                await session.turn(EMERGENCY);
                const release = Bun.sleep(1_500).then(() =>
                    session.h.host.releaseBlockedBackendCall(),
                );
                const emergency = await session.turn(EMERGENCY);
                expect(await release).toBe(true);

                expect(emergency.pass?.emergencyWaitMs ?? 0).toBeGreaterThan(0);
                const { firings, counters, passesByReason } = emergency.timing;
                expect(firings).toHaveLength(2);
                expect(activated(firings[1])).toBe(true);
                expect(counters?.exact.firings).toBe(2);
                expect(counters?.exact.published).toBe(2);
                // The rerun appends a second ring entry to the emergency request; grouping keeps it one response.
                const ring = (await session.status()).pass_trace as {
                    scheduler_history?: unknown[];
                };
                const grouped = Object.values(passesByReason).reduce((sum, n) => sum + n, 0);
                expect(grouped).toBe(session.h.readRustPasses().length);
                expect(ring.scheduler_history?.length).toBe(grouped + 1);
                printReport(await session.report(`${SCENARIO}:emergency`, runId));
            });
        },
        CASE_TIMEOUT_MS,
    );

    it.skipIf(!active)(
        "counts an unrendered publication a revert removes and reconciles through HARD",
        async () => {
            await withSession(2, async (session) => {
                await session.foldFirst(LEAD_2_HOLD);
                await session.turnsUntil(
                    LEAD_2_HOLD,
                    (firings) => firings[1]?.state === "pending",
                    "pending publication",
                );
                const before = (await session.timing()).counters;
                // The counter counts segments, so the pending publication's segment count is the expected rise; truncate clears the sequence, so read it first.
                const [first, pending] = await session.rawFirings();
                const segments =
                    (pending?.outcome?.sequence ?? 0) - (first?.outcome?.sequence ?? 0);
                expect(segments).toBeGreaterThan(0);

                await session.revertAfterFirstSegment();
                const observing = await session.turn(LEAD_2_HOLD);
                expect(observing.pass?.decision).toBe("SOFT+");
                const reconcile = await session.turn(LEAD_2_HOLD);
                expect(reconcile.pass?.decision).toBe("HARD");
                expect(reconcile.pass?.reason).toBe("reconcile");

                expect(reconcile.timing.firings[1]?.state).toBe("superseded");
                expect(reconcile.timing.counters?.exact.supersededBeforeActivation).toBe(
                    (before?.exact.supersededBeforeActivation ?? 0) + segments,
                );
                printReport(await session.report(`${SCENARIO}:pending-revert`, runId));
            });
        },
        CASE_TIMEOUT_MS,
    );

    it.skipIf(!active)(
        "rejects a publication the revert fence invalidated",
        async () => {
            await withSession(2, async (session) => {
                await session.foldFirst(LEAD_2_HOLD);
                await session.h.host.blockNextBackendCall();
                await session.turnsUntil(LEAD_2_HOLD, (firings) => firings.length >= 2, "firing");

                await session.revertAfterFirstSegment();
                await session.turn(LEAD_2_HOLD);
                const reconcile = await session.turn(LEAD_2_HOLD);
                expect(reconcile.pass?.decision).toBe("HARD");
                const before = reconcile.timing.counters?.bestEffort.invalidated ?? 0;

                expect(await session.h.host.releaseBlockedBackendCall()).toBe(true);
                const firings = await session.waitForFirings(
                    (entries) => entries[1]?.state === "abandoned",
                    "fenced publication",
                );
                expect(firings[1]?.abandonClass).toBe("invalidated");
                const counters = (await session.timing()).counters;
                expect(counters?.bestEffort.invalidated).toBe(before + 1);
                expect(counters?.exact.published).toBe(1);
                printReport(await session.report(`${SCENARIO}:revert-fence`, runId));
            });
        },
        CASE_TIMEOUT_MS,
    );
});
