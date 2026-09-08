import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { RustTestHarness } from "../src/rust-harness";
import {
    assertLoudModuleFailure,
    assertMessagesHaveNoPlaceholders,
    driveToSteadyState,
    rustPrereqs,
} from "../src/rust-scenario-support";

const OUTAGE_PASSES = 4;

describe.skipIf(!rustPrereqs.ok)("rust failure-mode drill FM-OC-2: host outage", () => {
    let h: RustTestHarness;

    beforeEach(async () => {
        h = await RustTestHarness.create({
            modelContextLimit: 100_000,
            eidnaraConfig: { execute_threshold_percentage: 40, protected_tags: 1 },
        });
    });

    afterEach(async () => {
        await h?.dispose();
    });

    it("continues through the outage with a loud module failure", async () => {
        const sessionId = await h.createSession();
        await driveToSteadyState(h, sessionId, 2);
        const beforeCount = h.readRustPasses().length;

        await h.host.crashHost();
        for (let i = 1; i <= OUTAGE_PASSES; i += 1) {
            h.mock.setDefault({
                text: `FM-OC-2 outage assistant ${i}`,
                usage: {
                    input_tokens: 2_000 * i,
                    output_tokens: 20,
                    cache_creation_input_tokens: 1_000,
                },
            });
            await h.sendPrompt(sessionId, `FM-OC-2 outage turn ${i}: ${h.ballast(400)}`);
            assertMessagesHaveNoPlaceholders(h.lastMainMessages(), sessionId);
        }

        const passes = await h.waitForRustPasses(beforeCount + OUTAGE_PASSES);
        const outage = passes.slice(beforeCount);
        expect(outage.length).toBeGreaterThanOrEqual(OUTAGE_PASSES);
        // The failure path serves the input unchanged and labels the pass `raw`.
        expect(outage.every((pass) => pass.servedFrom === "raw")).toBe(true);

        assertLoudModuleFailure(h, sessionId);
    }, 300_000);
});
