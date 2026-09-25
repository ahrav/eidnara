/**
 * The compaction-timing scenario: fixed-lead arms through fire, controlled producer delay, pending wait, and activation, plus emergency growth, the two revert cases, and one replay of the sweep workload for the arm `EIDNARA_E2E_TIMING_ARM` names (`default` when unset). Every durable claim is read back from `session.status`; served actions and emergency waits come from the plugin's `rust pass:` lines. Each case asserts the branch it names was reached, so a run cannot pass without exercising it.
 */

import { describe, expect, it } from "bun:test";
import {
    activated,
    EXECUTE_THRESHOLD,
    printReport,
    published,
    runSweepArm,
    SWEEP,
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
/** Below the execute threshold and at or above both arms' proactive percentage (lead 2: 63). */
const HOLD = 64;
/** Every lead arm climbs these steps after the same quiet phase; lead 10 fires on the first, lead 2 only on the last. */
const STEPS = [56, 60, 64] as const;
const STEP_TURNS = 3;
const EXECUTE = EXECUTE_THRESHOLD + 1;
const EMERGENCY = 96;
/** Below every arm's proactive percentage, so turns there grow the eligible head without firing. */
const QUIET = 50;
const QUIET_TURNS = 8;
const PRODUCER_DELAY_TURNS = 2;
/** The held producer finishes this long into an emergency wait. */
const EMERGENCY_FINISH_MS = 1_500;

const runId = `${SCENARIO}-${process.pid}`;
const active = rustPrereqs.ok && foldInfraEnabled();
/** `default` or a lead; the sweep arm the gated run replays. */
const sweepArm = process.env.EIDNARA_E2E_TIMING_ARM ?? "default";

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

/** First fold, then a quiet phase that grows the eligible head without firing. */
async function foldThenQuiet(session: TimingSession): Promise<void> {
    await session.foldFirst(HOLD);
    for (let turn = 0; turn < QUIET_TURNS; turn += 1) {
        const quiet = await session.turn(QUIET);
        expect(quiet.timing.firings).toHaveLength(1);
    }
}

/** The identical workload for every lead: climb the steps with the producer held until the measured firing starts, publish after a controlled delay, wait pending below the threshold, activate on the first Execute pass. Returns the measured firing's usage. */
async function leadArm(session: TimingSession, label: string): Promise<number> {
    await foldThenQuiet(session);
    await session.h.host.blockNextBackendCall();
    let step: number | undefined;
    for (const percent of STEPS) {
        for (let turn = 0; turn < STEP_TURNS && step === undefined; turn += 1) {
            const climb = await session.turn(percent);
            if (climb.timing.firings.length >= 2) step = percent;
        }
        if (step !== undefined) break;
    }
    if (step === undefined) throw new Error(`${label}: no measured firing on ${STEPS.join(", ")}`);
    const usage = (await session.rawFirings())[1]?.usage?.usage_percentage;
    if (usage === undefined) throw new Error("measured firing recorded no usage");

    for (let turn = 0; turn < PRODUCER_DELAY_TURNS; turn += 1) {
        const held = await session.turn(step);
        expect(published(held.timing.firings[1])).toBe(false);
    }
    expect(await session.h.host.releaseBlockedBackendCall()).toBe(true);
    await session.waitForFirings((firings) => published(firings[1]), "measured publication");

    const waiting = await session.turn(step);
    expect(waiting.pass?.decision).toBe("SOFT+");
    expect(waiting.timing.firings[1]?.state).toBe("pending");

    // The pass that serves this turn still reads the held pressure; the next pass is the first at Execute.
    const reported = await session.turn(EXECUTE);
    expect(activated(reported.timing.firings[1])).toBe(false);
    const execute = await session.turn(EXECUTE);
    expect(activated(execute.timing.firings[1])).toBe(true);
    // Which rebuild reason wins the activating pass varies with the transcript; it is a rebuild either way.
    expect(["SOFT", "HARD"]).toContain(execute.pass?.decision ?? "");
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
                lead2 = await leadArm(session, "lead-2");
            });
            // Lead 2 stays quiet through 56 and 60 and fires only at the last step.
            expect(lead2).toBeGreaterThanOrEqual(EXECUTE_THRESHOLD - 2);
            expect(lead2).toBeLessThan(EXECUTE_THRESHOLD);

            let lead10 = 0;
            await withSession(10, async (session) => {
                lead10 = await leadArm(session, "lead-10");
            });
            expect(lead10).toBeGreaterThanOrEqual(EXECUTE_THRESHOLD - 10);
            expect(lead10).toBeLessThan(EXECUTE_THRESHOLD - 2);
            expect(lead10).toBeLessThan(lead2);
        },
        CASE_TIMEOUT_MS,
    );

    it.skipIf(!active)(
        "waits at emergency pressure on one inline firing and counts its rerun once",
        async () => {
            await withSession(2, async (session) => {
                await foldThenQuiet(session);
                // The emergency pass fires inline and waits on the held producer until the release.
                await session.h.host.blockNextBackendCall();
                await session.turn(EMERGENCY);
                const release = Bun.sleep(EMERGENCY_FINISH_MS).then(() =>
                    session.h.host.releaseBlockedBackendCall(),
                );
                let emergency: Awaited<ReturnType<TimingSession["turn"]>>;
                try {
                    emergency = await session.turn(EMERGENCY);
                } finally {
                    await release.catch(() => false);
                }
                expect(await release).toBe(true);

                expect(emergency.pass?.emergencyWaitMs ?? 0).toBeGreaterThanOrEqual(
                    EMERGENCY_FINISH_MS - 500,
                );
                const { firings, counters, passesByReason, emergencyPasses } = emergency.timing;
                expect(emergencyPasses).toBe(1);
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
                await session.foldFirst(HOLD);
                await session.turnsUntil(
                    HOLD,
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
                const observing = await session.turn(HOLD);
                expect(observing.pass?.decision).toBe("SOFT+");
                const reconcile = await session.turn(HOLD);
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
                await session.foldFirst(HOLD);
                await session.h.host.blockNextBackendCall();
                await session.turnsUntil(HOLD, (firings) => firings.length >= 2, "firing");

                await session.revertAfterFirstSegment();
                await session.turn(HOLD);
                const reconcile = await session.turn(HOLD);
                expect(reconcile.pass?.decision).toBe("HARD");
                expect(reconcile.pass?.reason).toBe("reconcile");
                const before = reconcile.timing.counters?.bestEffort.invalidated ?? 0;

                expect(await session.h.host.releaseBlockedBackendCall()).toBe(true);
                const firings = await session.waitForFirings(
                    (entries) => entries[1]?.state === "abandoned",
                    "fenced publication",
                );
                expect(firings[1]?.abandonClass).toBe("invalidated");
                // The selected-range identities survive the revert, so the revert-epoch check is the fence that fires.
                const summarizer = (await session.durableStatus()).history_summarizer as {
                    last_failure?: string;
                };
                expect(summarizer.last_failure).toContain("revert epoch mismatch");
                const counters = (await session.timing()).counters;
                expect(counters?.bestEffort.invalidated).toBe(before + 1);
                expect(counters?.exact.published).toBe(1);
                printReport(await session.report(`${SCENARIO}:revert-fence`, runId));
            });
        },
        CASE_TIMEOUT_MS,
    );

    it.skipIf(!active)(
        `replays the sweep workload for the ${sweepArm} arm`,
        async () => {
            const lead = sweepArm === "default" ? undefined : Number(sweepArm);
            if (lead !== undefined && !Number.isInteger(lead)) {
                throw new Error(`EIDNARA_E2E_TIMING_ARM must be "default" or an integer lead`);
            }
            const report = await runSweepArm(lead, runId);
            printReport(report);
            expect(report.passes).toBeGreaterThanOrEqual(SWEEP.turns);
            expect(report.pressure_path_firings).toBeGreaterThan(0);
            // The workload reaches emergency pressure at the default lead; a candidate lead may remove it.
            if (lead === undefined) expect(report.emergency_passes).toBeGreaterThan(0);
        },
        CASE_TIMEOUT_MS,
    );
});
