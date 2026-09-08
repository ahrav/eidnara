/**
 * Anthropic rejects requests that modify a `thinking` or `redacted_thinking` block in the latest
 * assistant message. Each case drives the transform through a situation that can touch such a
 * block and verifies the wire through the incident-pool verifiers.
 */

import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import {
    driveThinkingDroppedShell,
    driveThinkingImageSurvival,
    driveThinkingNudgeAnchor,
    failedCheckIds,
    THINKING_BLOCK_HARNESS_OPTIONS,
    verifyThinkingDroppedShell,
    verifyThinkingImageSurvival,
    verifyThinkingNudgeAnchor,
} from "../src/incident-pool/scenarios/source-linked-regressions";
import { RustTestHarness } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";

describe.skipIf(!rustPrereqs.ok)("thinking-block safety (Anthropic 400 regression)", () => {
    let h: RustTestHarness;

    beforeAll(async () => {
        h = await RustTestHarness.create(THINKING_BLOCK_HARNESS_OPTIONS);
    });

    afterAll(async () => {
        await h?.dispose();
    });

    describe("Bug A: nudge anchor on a thinking-bearing assistant", () => {
        it("does not inject nudge <instruction> text into an assistant that has a thinking block", async () => {
            const observation = await driveThinkingNudgeAnchor(h);
            const result = verifyThinkingNudgeAnchor(observation);
            expect(failedCheckIds(result)).toEqual([]);
            expect(result.verdict).toBe("pass");
        }, 90_000);
    });

    describe("Bug B: user-message turn boundary preserved when text tag is dropped", () => {
        it("keeps provider roles safe when whole-arc history supersedes the dropped shell", async () => {
            const observation = await driveThinkingDroppedShell(h);
            expect(observation.dropEmitted).toBe(true);
            const result = verifyThinkingDroppedShell(observation);
            expect(failedCheckIds(result)).toEqual([]);
            expect(result.verdict).toBe("pass");
        }, 120_000);
    });

    describe("Bug C: file/image part survives when companion text is dropped", () => {
        it("allows whole-arc history to supersede the image without partial stripping", async () => {
            const observation = await driveThinkingImageSurvival(h);
            expect(observation.dropEmitted).toBe(true);
            const result = verifyThinkingImageSurvival(observation);
            expect(failedCheckIds(result)).toEqual([]);
            expect(result.verdict).toBe("pass");
        }, 90_000);
    });
});
