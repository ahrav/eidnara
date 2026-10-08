/**
 * Ungated fixture qualification for compression fidelity delivery.
 *
 * One corpus scenario is scripted through the direct-host fixture's case queue, its native
 * records are seeded into the OpenCode session, and the session is driven until the daemon
 * publishes the bound approved example. The next provider request for the same session and
 * follow-up must carry the published history: accepted publication, an applied recipe on the
 * pass line, a nonempty correlated capture whose history serves the reviewed P1 body, and none of
 * the covered native text in the live tail. Campaign delivery claims depend on this test passing
 * in the manifest-selected run.
 */

import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import {
    COMPRESSION_FIDELITY_CORPUS_SHA256,
    readCompressionFidelityCorpus,
} from "../src/compression-fidelity/corpus";
import {
    captureTexts,
    emitObservation,
    judgeDelivery,
    publishedCount,
    reviewedTiers,
    seedSource,
    sessionPasses,
} from "../src/compression-fidelity/delivery";
import { waitFor } from "../src/harness-primitives";
import { RustTestHarness, requestSessionId } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";

/** Set in CI, where a skipped qualification would hide a blocking runtime prerequisite. */
const QUALIFICATION_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_FIDELITY === "1";

const EIDNARA_CONFIG = {
    history_summarizer: { model: "fixture/deterministic" },
    execute_threshold_percentage: 25,
    protected_tags: 1,
    memory: { auto_search: { enabled: false } },
};

const CASE = "C1";
const SCENARIO = "C1.S1";
/** Trailing turns allowed to reach the summarizer trigger before qualification fails. */
const MAX_TRAILING_TURNS = 12;
const WAIT_MS = 120_000;

describe("compression fidelity: fixture qualification prerequisites", () => {
    it.skipIf(!QUALIFICATION_REQUIRED)("are present when EIDNARA_E2E_REQUIRE_FIDELITY=1", () => {
        expect(rustPrereqs.skipReason ?? "present").toBe("present");
    });
});

describe.skipIf(!rustPrereqs.ok)("compression fidelity: fixture qualification", () => {
    let h: RustTestHarness;

    beforeAll(async () => {
        h = await RustTestHarness.create({
            modelContextLimit: 30_000,
            eidnaraConfig: EIDNARA_CONFIG,
        });
    }, 600_000);

    afterAll(async () => {
        await h?.dispose();
    });

    it("publishes a scripted corpus case and serves it in a correlated provider request", async () => {
        const fidelityCase = readCompressionFidelityCorpus().cases.find(
            (entry) => entry.id === CASE,
        );
        const scenario = fidelityCase?.scenarios.find((entry) => entry.id === SCENARIO);
        const reviewed = fidelityCase?.sources.find((entry) => entry.id === scenario?.source);
        if (!scenario || !reviewed) throw new Error(`${SCENARIO} is not in the corpus`);
        const { title, bodies } = reviewedTiers(reviewed.reviewedOutput);

        const sessionId = await h.createSession();
        h.tagCaptures({ sessionId, caseId: CASE, scenarioId: SCENARIO });
        // One completed turn gives the seeded records their OpenCode message shape.
        h.mock.setDefault({
            text: "lead-in answer",
            usage: { input_tokens: 500, output_tokens: 20 },
        });
        await h.sendPrompt(sessionId, "lead-in turn");

        const source = await h.host.scriptSource(SCENARIO);
        expect(source.corpusSha256).toBe(COMPRESSION_FIDELITY_CORPUS_SHA256);
        expect(source.source).toBe(reviewed.id);
        expect(source.leakProbes.length).toBeGreaterThan(0);
        seedSource(h, sessionId, source);
        await h.restart({ eidnaraConfig: EIDNARA_CONFIG });
        await h.host.scriptCases([SCENARIO, "filler", "filler", "filler"]);

        // Newer turns move the seeded source out of the protected tail and past the trigger.
        for (let i = 1; i <= MAX_TRAILING_TURNS; i += 1) {
            h.mock.setDefault({
                text: `trailing assistant ${i}`,
                usage: { input_tokens: 3_000 * i, output_tokens: 20 },
            });
            await h.sendPrompt(sessionId, `trailing turn ${i}: ${h.ballast(2_000)}`);
            const status = await h.host.scriptStatus();
            if (status.remaining < 4) break;
        }
        const diagnostics = () => h.host.hostLog().slice(-4_000);
        // The firing that consumed the case entry answers asynchronously.
        const script = await waitFor(
            "the scripted case to be answered",
            async () => {
                const status = await h.host.scriptStatus();
                return status.bound + status.mismatched + status.filled > 0 ? status : undefined;
            },
            WAIT_MS,
            diagnostics,
        );
        expect(script).toMatchObject({
            corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
            bound: 1,
            mismatched: 0,
            exhausted: 0,
        });
        expect(script.bindings[0]).toMatchObject({ scenario: SCENARIO, source: source.source });
        await waitFor(
            "the bound case to publish",
            async () => ((await publishedCount(h, sessionId)) >= 1 ? true : undefined),
            WAIT_MS,
            diagnostics,
        );
        await waitFor(
            "the summarizer to settle",
            async () => ((await h.host.historySummarizerLive()) ? undefined : true),
            WAIT_MS,
            diagnostics,
        );

        const passesBefore = sessionPasses(h, sessionId).length;
        h.mock.setDefault({
            text: "follow-up answer",
            usage: { input_tokens: 9_000, output_tokens: 20 },
        });
        await h.sendPrompt(sessionId, scenario.followUp.prompt);
        const pass = await waitFor(
            "the follow-up pass line",
            () => sessionPasses(h, sessionId)[passesBefore],
            WAIT_MS,
            diagnostics,
        );
        const capture = h
            .retainedCaptures({ sessionId, scenarioId: SCENARIO })
            .filter((entry) => captureTexts(entry).at(-1)?.includes(scenario.followUp.prompt))
            .at(-1);
        const verdict = judgeDelivery({
            capture,
            pass,
            title,
            bodies,
            leakProbes: source.leakProbes,
        });
        emitObservation({
            case: CASE,
            source: source.source,
            scenario: SCENARIO,
            stage: "qualification",
            terminal: verdict.refusals.length === 0 ? "served" : "unqualified",
            markers: verdict.refusals.length === 0 ? ["cf-fixture-script-qualification"] : [],
            detail: {
                binding: script.bindings[0],
                pass: {
                    decision: pass.decision,
                    reason: pass.reason,
                    served_from: pass.servedFrom,
                    applied: pass.applied,
                },
                served_tier: verdict.tier,
                refusals: verdict.refusals,
                leaks: verdict.leaks,
                capture_messages: capture ? captureTexts(capture).length : 0,
            },
        });

        expect(verdict.refusals).toEqual([]);
        expect(verdict.tier).toBe("p1");
        expect(pass.decision).toMatch(/^[A-Z]+\+?$/);
        expect(pass.reason).toMatch(/^[a-z_]+$/);
        expect(capture ? requestSessionId(capture.request) : undefined).toBe(sessionId);
        expect(capture?.caseId).toBe(CASE);
    }, 600_000);
});
