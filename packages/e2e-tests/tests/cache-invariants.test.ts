/**
 * A stale `ctx_reduce` strip requires growth, an EXECUTE pass that freezes drop state, and a
 * subsequent DEFER pass. DEFER passes must not change the cached prefix. `findBusts` counts
 * changes between consecutive requests to a wire segment before the final `cache_control`
 * breakpoint.
 *
 *   A1  low-pressure pure-defer growth stays byte-stable
 *   A2  defer passes AFTER an execute pass + growth stay byte-stable
 *   A3  an aged ctx_reduce call never vanishes mid-prefix on a defer pass
 */

import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { findBusts, formatBustReport, mainAgentRequests } from "../src/cache-analysis";
import {
    driveAgedCtxReduceSurvival,
    driveFirstRenderPureDeferStability,
    FIRST_RENDER_HARNESS_OPTIONS,
    failedCheckIds,
    verifyAgedCtxReduceSurvival,
    verifyFirstRenderPureDeferStability,
} from "../src/incident-pool/scenarios/source-linked-regressions";
import type { MockUsage } from "../src/mock-provider/server";
import { RustTestHarness } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";

// Usage below `execute_threshold` (20,000 tokens) causes a DEFER pass.
const DEFER_USAGE: MockUsage = {
    input_tokens: 2_000,
    output_tokens: 20,
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: 2_000,
};

// Usage above `execute_threshold` causes the next pass to execute.
const EXECUTE_USAGE: MockUsage = {
    input_tokens: 30_000,
    output_tokens: 20,
    cache_creation_input_tokens: 30_000,
    cache_read_input_tokens: 0,
};

describe.skipIf(!rustPrereqs.ok)("cache invariants — replay class", () => {
    let h: RustTestHarness;

    beforeEach(async () => {
        h = await RustTestHarness.create(FIRST_RENDER_HARNESS_OPTIONS);
    });

    afterEach(async () => {
        await h?.dispose();
    });

    function setDefer(text: string): void {
        h.mock.setDefault({ text, usage: DEFER_USAGE });
    }

    describe("#given a low-pressure conversation (A1)", () => {
        describe("#when several pure-defer turns grow the tail", () => {
            it("#then the cached prefix never busts across defer passes", async () => {
                const observation = await driveFirstRenderPureDeferStability(h);

                if (observation.bustReport) {
                    console.error(
                        `[cache-invariant:A1-low-pressure-defer] ${observation.bustCount} bust(s):\n${observation.bustReport}`,
                    );
                }
                expect(observation.mainRequestCount).toBeGreaterThanOrEqual(6);
                const result = verifyFirstRenderPureDeferStability(observation);
                expect(failedCheckIds(result)).toEqual([]);
                expect(result.verdict).toBe("pass");
            }, 120_000);
        });
    });

    describe("#given a conversation that crossed an execute pass (A2)", () => {
        describe("#when defer passes follow the execute pass with continued growth", () => {
            it("#then defer passes after the execute settle to a stable prefix", async () => {
                // A high-usage turn after warm-up makes the next transform execute.
                const sessionId = await h.createSession();
                setDefer("A2 warmup 1");
                await h.sendPrompt(sessionId, "A2 turn 1: warmup.");
                setDefer("A2 warmup 2");
                await h.sendPrompt(sessionId, "A2 turn 2: warmup.");

                h.mock.setDefault({ text: "A2 high usage", usage: EXECUTE_USAGE });
                await h.sendPrompt(sessionId, "A2 turn 3: high usage triggers an execute pass.");

                // The execute pass busts once when drops and markers materialize.
                // DEFER passes following the execute pass are byte-stable.
                const firstDeferIndex = h.mock.requests().length;
                for (let i = 4; i <= 8; i++) {
                    setDefer(`A2 defer reply ${i}`);
                    await h.sendPrompt(sessionId, `A2 turn ${i}: defer growth after execute.`);
                }

                // `findBusts` evaluates only the post-execute DEFER window.
                const deferRequests = mainAgentRequests(h.mock.requests().slice(firstDeferIndex));
                expect(deferRequests.length).toBeGreaterThanOrEqual(4);
                const busts = findBusts(deferRequests);
                if (busts.length > 0) {
                    console.error(
                        `[cache-invariant:A2-post-execute-defer] ${busts.length} bust(s):\n${formatBustReport(busts)}`,
                    );
                }
                expect(busts.length).toBe(0);
            }, 150_000);
        });
    });

    describe("#given an aged ctx_reduce call in the conversation (A3 — the regression)", () => {
        describe("#when pure-defer turns grow the tail past the protected window", () => {
            it("#then the ctx_reduce message never vanishes mid-prefix and the prefix never busts", async () => {
                const observation = await driveAgedCtxReduceSurvival(h);

                if (observation.bustReport) {
                    console.error(
                        `[cache-invariant:A3-ctx_reduce-defer-growth] ${observation.bustCount} bust(s):\n${observation.bustReport}`,
                    );
                }
                expect(observation.sawReduceOnWire).toBe(true);
                expect(observation.finalWireHasCtxReduce).toBe(true);
                const result = verifyAgedCtxReduceSurvival(observation);
                expect(failedCheckIds(result)).toEqual([]);
                expect(result.verdict).toBe("pass");
            }, 150_000);
        });
    });
});
