import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { type RustPassLine, RustTestHarness } from "../src/rust-harness";
import { RustPassCursor } from "../src/rust-pass-cursor";
import {
    assertLoudModuleFailure,
    assertMessagesHaveNoPlaceholders,
    driveToSteadyState,
    rustPrereqs,
} from "../src/rust-scenario-support";

const OUTAGE_PASSES = 4;
const RECOVERY_PASSES = 6;

describe.skipIf(!rustPrereqs.ok)("rust failure-mode drill FM-OC-3: self-heal after outage", () => {
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

    it("serves passes from transform again after the host restarts, without restarting the session", async () => {
        const sessionId = await h.createSession();
        await driveToSteadyState(h, sessionId, 2);
        const healthyVersions = h
            .readRustPasses()
            .map((pass) => pass.rowVersion)
            .filter((version) => version > 0);

        await h.host.crashHost();
        for (let i = 1; i <= OUTAGE_PASSES; i += 1) {
            h.mock.setDefault({
                text: `FM-OC-3 outage assistant ${i}`,
                usage: {
                    input_tokens: 2_000 * i,
                    output_tokens: 20,
                    cache_creation_input_tokens: 1_000,
                },
            });
            await h.sendPrompt(sessionId, `FM-OC-3 outage turn ${i}: ${h.ballast(400)}`);
        }
        assertLoudModuleFailure(h, sessionId);

        await h.host.restartHost();
        // The settled baseline keeps a buffered outage pass out of the recovery window.
        const passCursor = await RustPassCursor.settle(h);
        const recovery: RustPassLine[] = [];
        for (let i = 1; i <= RECOVERY_PASSES; i += 1) {
            h.mock.setDefault({
                text: `FM-OC-3 recovery assistant ${i}`,
                usage: {
                    input_tokens: 2_000 * (OUTAGE_PASSES + i),
                    output_tokens: 20,
                    cache_creation_input_tokens: 1_000,
                },
            });
            await h.sendPrompt(sessionId, `FM-OC-3 recovery turn ${i}: ${h.ballast(400)}`);
            // A prompt's pass lines are produced before it returns, so the settled read ties each
            // of them to this turn: every turn logs one pass, a reconnect retry repeats it once,
            // and a turn served unchanged without a pass cannot hide behind another turn's retry.
            const turn = await passCursor.settledStep();
            expect(turn.length, `recovery turn ${i} logged a pass`).toBeGreaterThanOrEqual(1);
            expect(turn.length, `recovery turn ${i} retried at most once`).toBeLessThanOrEqual(2);
            recovery.push(...turn);
        }

        const firstRecovered = recovery.findIndex((pass) => pass.servedFrom === "transform");
        expect(firstRecovered).toBeGreaterThanOrEqual(0);
        // Self-healing holds only if no pass falls back to raw after the transform first serves one.
        expect(
            recovery.slice(firstRecovered).every((pass) => pass.servedFrom === "transform"),
        ).toBe(true);
        expect(recovery.at(-1)?.servedFrom).toBe("transform");

        // Recovery row versions must continue the healthy session's persisted lineage.
        const recoveryVersions = recovery
            .map((pass) => pass.rowVersion)
            .filter((version) => version > 0);
        expect(recoveryVersions.length).toBeGreaterThan(0);
        const allVersions = [...healthyVersions, ...recoveryVersions];
        expect(
            allVersions.every(
                (version, index) => index === 0 || version >= allVersions[index - 1]!,
            ),
        ).toBe(true);
        expect(recoveryVersions.at(-1)).toBeGreaterThan(healthyVersions.at(-1) ?? 0);
        assertMessagesHaveNoPlaceholders(h.lastMainMessages(), sessionId);
    }, 300_000);
});
