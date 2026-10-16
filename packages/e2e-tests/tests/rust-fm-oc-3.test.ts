import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { RustTestHarness } from "../src/rust-harness";
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
        const recoveryStart = h.readRustPasses().length;
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
        }

        // Every recovery prompt has returned, so its pass line is due; waiting for all of them keeps a late raw fallback observable.
        const passes = await h.waitForRustPasses(recoveryStart + RECOVERY_PASSES);
        const recovery = passes.slice(recoveryStart);
        // A reconnect retry repeats the pass for the prompt it retries, so the ceiling counts
        // prompts: passes group by the input they served, one group per prompt, and at most
        // one retry joins each.
        const passesPerPrompt = new Map<number, number>();
        for (const pass of recovery) {
            passesPerPrompt.set(pass.inputCount, (passesPerPrompt.get(pass.inputCount) ?? 0) + 1);
        }
        expect(passesPerPrompt.size).toBeLessThanOrEqual(RECOVERY_PASSES);
        for (const count of passesPerPrompt.values()) expect(count).toBeLessThanOrEqual(2);
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
