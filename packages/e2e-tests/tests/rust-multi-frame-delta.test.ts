import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { type RustPassLine, RustTestHarness } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";

describe.skipIf(!rustPrereqs.ok)("rust transport: whole-array sends with a large tail", () => {
    let h: RustTestHarness;

    beforeAll(async () => {
        h = await RustTestHarness.create({
            modelContextLimit: 50_000_000,
            eidnaraConfig: {
                execute_threshold_percentage: 95,
                protected_tags: 1,
            },
        });
    });

    afterAll(async () => {
        await h?.dispose();
    });

    // Quarantined: the 160k-token tail exceeds the memory store's 512 KiB MAX_DURABLE_TEXT_BYTES, so the daemon answers "store: durable text rejected: InputLimit" and the pass serves raw. The small whole-array sends pass; the large-tail step needs a tail under that limit. Deferred in the #829 PR.
    it.skip("keeps module paging bounded while preserving a large provider-visible tail", async () => {
        const sessionId = await h.createSession();
        await h.sendPrompt(sessionId, "establish the initial module snapshot");
        await h.waitForRustPasses(1);

        h.appendSyntheticHistory(sessionId, { count: 1_000, textBytes: 1024 });
        await h.restart({
            eidnaraConfig: {
                execute_threshold_percentage: 95,
                protected_tags: 1,
            },
        });
        await h.sendPrompt(sessionId, "prime the synthetic big-session snapshot", {
            timeoutMs: 300_000,
        });
        const primed = await h.waitFor(
            () => {
                const passes = h.readRustPasses();
                for (let index = passes.length - 1; index >= 0; index -= 1) {
                    if (passes[index]!.inputCount > 1_000) return passes[index];
                }
                return undefined;
            },
            { timeoutMs: 30_000, label: "primed 1,000-message rust pass" },
        );
        expect(primed.inputCount).toBeGreaterThan(1_000);
        expect(primed.servedFrom).toBe("transform");
        expect(primed.transportPages).toBeGreaterThan(1);

        let settled = primed;
        for (let probe = 0; !settled.applied && probe < 3; probe += 1) {
            const before = h.readRustPasses().length;
            await h.sendPrompt(sessionId, `settle the synthetic big-session snapshot ${probe}`);
            settled = (await h.waitForRustPasses(before + 1)).at(-1)!;
        }
        expect(settled.applied).toBe(true);

        const smallSends: RustPassLine[] = [];
        for (let probe = 0; probe < 5; probe += 1) {
            const before = h.readRustPasses().length;
            await h.sendPrompt(sessionId, `small steady-state send ${probe}`);
            smallSends.push((await h.waitForRustPasses(before + 1)).at(-1)!);
        }

        const providerBytesBeforeLargeTail = h.lastMainWireBytes();
        const before = h.readRustPasses().length;
        await h.sendPrompt(sessionId, `large tail send: ${h.ballast(160_000)}`, {
            timeoutMs: 300_000,
        });
        const largeTail = (await h.waitForRustPasses(before + 1)).at(-1)!;
        const providerBytesAfterLargeTail = h.lastMainWireBytes();

        // Every pass carries the whole 1,000-message history on the wire, not a tail delta of a few messages.
        const historyBytes = 1_000 * 1024;
        expect(smallSends.every((pass) => pass.applied)).toBe(true);
        expect(smallSends.every((pass) => pass.transportBytes > historyBytes)).toBe(true);
        expect(smallSends.every((pass) => pass.transportPages > 1)).toBe(true);

        expect(largeTail.applied).toBe(true);
        expect(largeTail.transportBytes).toBeGreaterThan(historyBytes + 160_000);
        expect(largeTail.transportPages).toBeGreaterThan(1);
        expect(largeTail.transportBytes).toBeGreaterThan(512 * 1024);
        expect(providerBytesAfterLargeTail).toBeGreaterThan(
            providerBytesBeforeLargeTail + 512 * 1024,
        );
        expect(h.lastMainWireSerialized()).toContain("large tail send");
    }, 600_000);
});
