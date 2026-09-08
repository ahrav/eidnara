import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { RustTestHarness } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";

/**
 * Eidnara keeps the Anthropic prompt cache alive across turns, so every defer-pass transform
 * must serve a byte-identical prefix: the system prompt and every prior message.
 *
 * OpenCode moves `cache_control` to the latest message each turn, so the comparison strips it.
 *
 *   1. The system field stays byte-identical across turns 2..N.
 *   2. Each earlier request's messages are a byte-identical prefix of the next request's.
 */

/** stripCacheControl removes `cache_control` because OpenCode moves it to the latest message each turn. */
function stripCacheControl(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(stripCacheControl);
    if (value && typeof value === "object") {
        const out: Record<string, unknown> = {};
        for (const [k, v] of Object.entries(value)) {
            if (k === "cache_control") continue;
            out[k] = stripCacheControl(v);
        }
        return out;
    }
    return value;
}

function serialize(value: unknown): string {
    return JSON.stringify(stripCacheControl(value));
}

const TURN_COUNT = 5;

/** The stability assertions require every turn to be served from the Rust transform. */
async function driveStableSession(h: RustTestHarness) {
    h.mock.reset();
    h.mock.setDefault({
        text: "ok",
        usage: {
            input_tokens: 200,
            output_tokens: 10,
            cache_creation_input_tokens: 100,
            cache_read_input_tokens: 100,
        },
    });

    const sessionId = await h.createSession();
    const passesBefore = h.readRustPasses().length;

    for (let i = 1; i <= TURN_COUNT; i++) {
        await h.sendPrompt(sessionId, `turn ${i}: probe message for cache stability.`);
    }

    const passes = (await h.waitForRustPasses(passesBefore + TURN_COUNT)).slice(passesBefore);
    expect(passes.length).toBeGreaterThanOrEqual(TURN_COUNT);
    for (const pass of passes) {
        expect(pass.decision).not.toBe("error");
        expect(pass.servedFrom).toBe("transform");
    }

    const mainRequests = h.mainRequests();
    expect(mainRequests.length).toBeGreaterThanOrEqual(TURN_COUNT);
    return mainRequests;
}

describe.skipIf(!rustPrereqs.ok)("cache stability", () => {
    let h: RustTestHarness;

    beforeAll(async () => {
        h = await RustTestHarness.create({
            eidnaraConfig: {
                execute_threshold_percentage: 80,
            },
        });
    });

    afterAll(async () => {
        await h?.dispose();
    });

    it("system prompt stays stable across defer passes", async () => {
        const mainRequests = await driveStableSession(h);

        // The test excludes turn 1 because it establishes the cache.
        // The comparison strips `cache_control` because OpenCode moves it to the latest message each turn.
        const systems = new Set<string>();
        for (let i = 1; i < mainRequests.length; i++) {
            systems.add(serialize(mainRequests[i]!.body.system));
        }
        if (systems.size !== 1) {
            console.log(`[TEST] ${systems.size} distinct system variants`);
        }
        expect(systems.size).toBe(1);
    }, 60_000);

    it("prefix messages stay stable across defer passes", async () => {
        const mainRequests = await driveStableSession(h);

        // After stripping `cache_control`, each earlier request's messages must be a byte-identical prefix of the next request's messages.
        for (let i = 1; i < mainRequests.length - 1; i++) {
            const earlier = mainRequests[i]!.body.messages as unknown[];
            const later = mainRequests[i + 1]!.body.messages as unknown[];
            expect(earlier.length).toBeLessThanOrEqual(later.length);
            for (let j = 0; j < earlier.length; j++) {
                const earlierMsg = serialize(earlier[j]);
                const laterMsg = serialize(later[j]);
                if (earlierMsg !== laterMsg) {
                    console.log(`[TEST] prefix mismatch at turn pair ${i}/${i + 1} message ${j}:`);
                    console.log(`  earlier: ${earlierMsg.slice(0, 300)}`);
                    console.log(`  later:   ${laterMsg.slice(0, 300)}`);
                }
                expect(earlierMsg).toBe(laterMsg);
            }
        }
    }, 60_000);
});
