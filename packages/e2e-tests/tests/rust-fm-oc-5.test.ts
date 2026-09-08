import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { wireCarriesTagOverlay } from "../src/incident-pool/scenarios/source-linked-regressions";
import { RustTestHarness } from "../src/rust-harness";
import {
    assertLoudModuleFailure,
    assertMessagesHaveNoPlaceholders,
    driveToSteadyState,
    rustPrereqs,
} from "../src/rust-scenario-support";

describe.skipIf(!rustPrereqs.ok)("rust failure-mode drill FM-OC-5: transport hang", () => {
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

    it("continues through a transport timeout and recovers after SIGCONT", async () => {
        const sessionId = await h.createSession();
        await driveToSteadyState(h, sessionId, 2);
        const beforeCount = h.readRustPasses().length;

        await h.host.pauseHost();
        await h.sendPrompt(sessionId, `FM-OC-5 stopped module: ${h.ballast(400)}`);
        assertMessagesHaveNoPlaceholders(h.lastMainMessages(), sessionId);

        await h.host.resumeHost();
        await h.sendPrompt(sessionId, `FM-OC-5 continued module: ${h.ballast(400)}`);
        const recovered = await h.waitForRustPasses(beforeCount + 2);
        expect(
            recovered.slice(beforeCount + 1).some((pass) => pass.servedFrom === "transform"),
        ).toBe(true);
        // The pass log is per session, so recovery is confirmed on the resumed main request itself: a transform-served wire carries the tag overlay and the raw fallback does not.
        const resumedRequest = h.mainRequests().at(-1);
        expect(resumedRequest).toBeDefined();
        expect(wireCarriesTagOverlay(resumedRequest!.body)).toBe(true);

        // Outage passes serve the input unchanged, which the failure path labels `raw`.
        const lines = assertLoudModuleFailure(h, sessionId);
        expect(lines.some((line) => line.includes("served_from=raw"))).toBe(true);
        assertMessagesHaveNoPlaceholders(h.lastMainMessages(), sessionId);
    }, 300_000);
});
