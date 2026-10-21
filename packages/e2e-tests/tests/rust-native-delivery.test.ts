import { describe, expect, it } from "bun:test";
import { memoryId, occurrences, textOf } from "../src/pi-runner/rust-fixture";
import { RustTestHarness } from "../src/rust-harness";
import { buildDirectHostFixture, HermeticHostStack } from "../src/rust-runner/hermetic-host";
import { runScriptedToolCall } from "../src/scripted-tool-call";

const prereqs = RustTestHarness.detectPrereqs();

const PROJECT_MEMORY = "<project-memory>";
const MEMORY = { category: "PROJECT_RULES", text: "Serve every OpenCode message after m0 and m1." };

describe.skipIf(!prereqs.ok)("opencode knowledge delivery under native authority", () => {
    it("serves stored project memory once per request over three turns", async () => {
        const h = await RustTestHarness.createWithHost(
            { modelContextLimit: 200_000, openCodeConfigExtra: { compaction: { auto: true } } },
            async (env) =>
                HermeticHostStack.start({
                    dataDir: env.dataDir,
                    fixtureBin: await buildDirectHostFixture(),
                    daemonConfig: { memory: { auto_capture: false } },
                }),
        );
        try {
            const created = await runScriptedToolCall(h, await h.createSession(), {
                tool: "eidnara_memory",
                input: { action: "create", content: MEMORY.text, category: MEMORY.category },
                prompt: "Record this workspace convention as project memory.",
            });
            const anchor = /"objectId":"(mem_[0-9a-f]{32})"/.exec(created.resultText)?.[1];
            if (!anchor) throw new Error(`memory create returned no object: ${created.resultText}`);
            const id = memoryId("opencode-delivery");
            const admitted = await h.host.memorySeed(anchor, id, MEMORY.category, MEMORY.text);
            expect([admitted.effectiveMaturity, admitted.visibility]).toEqual([
                "verified",
                "automatic",
            ]);
            const line = `${id}: ${MEMORY.text}`;

            h.resetMock();
            h.mock.setDefault({
                text: "ok",
                usage: { input_tokens: 1_000, output_tokens: 10, cache_creation_input_tokens: 0 },
            });
            const sessionId = await h.createSession();
            for (const turn of [1, 2, 3]) {
                await h.sendPrompt(sessionId, `turn ${turn}`);
                const requests = h.mainRequests();
                expect(requests).toHaveLength(turn);
                const body = requests.at(-1)?.body ?? {};
                const messages = (body.messages ?? []) as unknown[];
                const first = textOf(messages[0]);
                expect(first).toContain("<session-history></session-history>");
                expect(occurrences(first, PROJECT_MEMORY)).toBe(1);
                expect(occurrences(JSON.stringify(messages), PROJECT_MEMORY)).toBe(1);
                expect(first).toContain(line);
                expect(occurrences(JSON.stringify(body), line)).toBe(1);
                expect(textOf(messages.at(-1))).toContain(`turn ${turn}`);
            }
            const others = h.mock
                .requests()
                .filter((request) => !h.mainRequests().includes(request));
            for (const request of others)
                expect(JSON.stringify(request.body)).not.toContain(PROJECT_MEMORY);
            // OpenCode titles the new session once; every other provider request is a turn.
            expect(others.length).toBeLessThanOrEqual(1);
            expect(await h.host.historySummarizerLive()).toBe(false);
            expect((await h.host.backendCounters()).started).toBe(0);
        } finally {
            await h.dispose();
        }
    }, 600_000);
});
