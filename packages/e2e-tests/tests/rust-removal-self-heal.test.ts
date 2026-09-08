/**
 * Removing a mid-session message through `session.revert` must not degrade the transform
 * permanently: later passes are served from `transform` again.
 */

import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { RustTestHarness } from "../src/rust-harness";
import { driveToSteadyState, rustPrereqs } from "../src/rust-scenario-support";

describe.skipIf(!rustPrereqs.ok)("rust incident regression: removal self-heal", () => {
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

    it("keeps transforming after a mid-session message is removed", async () => {
        const sessionId = await h.createSession();
        await driveToSteadyState(h, sessionId, 4);

        const passesBefore = h.readRustPasses();
        expect(passesBefore.some((p) => p.servedFrom === "transform")).toBe(true);
        expect(passesBefore.every((p) => p.decision !== "error")).toBe(true);

        // Choose a nonterminal user message so `session.revert` deletes later messages.
        // session.revert removes the selected message and every later message.
        const messages = await h.listMessages(sessionId);
        const userIds = messages
            .map((m) => m.info)
            .filter(
                (info): info is { id: string; role: string } =>
                    Boolean(info?.id) && info?.role === "user",
            )
            .map((info) => info.id);
        expect(userIds.length).toBeGreaterThanOrEqual(3);
        const midIndex = Math.floor(userIds.length / 2);
        const midUserId = userIds[midIndex]!;
        const survivingUserIds = userIds.slice(0, midIndex);
        const removedUserIds = userIds.slice(midIndex);

        await h.revertMessage(sessionId, midUserId);
        const deadline = Date.now() + 20_000;
        let remainingIds = new Set<string>();
        for (;;) {
            remainingIds = new Set(
                (await h.listMessages(sessionId)).flatMap((m) => (m.info?.id ? [m.info.id] : [])),
            );
            if (removedUserIds.every((id) => !remainingIds.has(id))) break;
            if (Date.now() >= deadline) {
                throw new Error(
                    `revert left ${removedUserIds.filter((id) => remainingIds.has(id)).join(", ")} in the session`,
                );
            }
            await Bun.sleep(100);
        }
        for (const id of survivingUserIds) expect(remainingIds.has(id)).toBe(true);

        const passCountBeforeNext = h.readRustPasses().length;

        for (let i = 6; i <= 10; i += 1) {
            h.mock.setDefault({
                text: `post-removal assistant ${i}`,
                usage: {
                    input_tokens: 2_000 * i,
                    output_tokens: 20,
                    cache_creation_input_tokens: 1_000,
                },
            });
            await h.sendPrompt(sessionId, `post-removal turn ${i}: ${h.ballast(400)}`);
            await Bun.sleep(400);
        }

        const allAfter = await h.waitForRustPasses(passCountBeforeNext + 3);
        const passesAfter = allAfter.slice(passCountBeforeNext);

        expect(passesAfter.some((p) => p.servedFrom === "transform")).toBe(true);
    }, 300_000);
});
