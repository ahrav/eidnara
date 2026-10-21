import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
    awaitStore,
    memoryId,
    occurrences,
    pluginModules,
    textOf,
    userConfig,
} from "../src/pi-runner/rust-fixture";
import { detectPiPrereqs } from "../src/pi-runner/spawn";
import {
    buildDirectHostFixture,
    detectRustModePrereqs,
    HermeticHostStack,
} from "../src/rust-runner/hermetic-host";

const rust = detectRustModePrereqs();
const pi = detectPiPrereqs();
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";
const active = rust.ok && pi.ok;

const COMPACTION_OFF = { compaction: { enabled: false } };
const PROJECT_MEMORY = "<project-memory>";
const FIRST = { category: "PROJECT_RULES", text: "Serve every Pi message after m0 and m1." };
const SECOND = {
    category: "CONSTRAINTS",
    text: "A memory written mid-session reaches the next pass.",
};

function assistantTurn(content: unknown[], stopReason: "stop" | "toolUse") {
    return {
        role: "assistant",
        content,
        api: "faux",
        provider: "faux",
        model: "faux-model",
        usage: {
            input: 0,
            output: 0,
            cacheRead: 0,
            cacheWrite: 0,
            totalTokens: 0,
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason,
        timestamp: Date.now(),
    };
}

type PiHarness = {
    session: {
        prompt(text: string): Promise<void>;
        compact(): Promise<unknown>;
        messages: { role?: string }[];
    };
    sessionManager: { getEntries(): { type: string; summary?: string }[] };
    requests: { messages: unknown[]; systemPrompt: string | undefined }[];
    respond(steps: unknown[]): void;
    dispose(): Promise<void>;
};

async function createdMemory(open: (options: Record<string, unknown>) => Promise<PiHarness>) {
    const setup = await open({ tools: ["eidnara_memory"] });
    try {
        setup.respond([
            assistantTurn(
                [
                    {
                        type: "toolCall",
                        id: "call-memory-create",
                        name: "eidnara_memory",
                        arguments: {
                            action: "create",
                            content: FIRST.text,
                            category: FIRST.category,
                        },
                    },
                ],
                "toolUse",
            ),
            assistantTurn([{ type: "text", text: "recorded" }], "stop"),
        ]);
        await setup.session.prompt("Record this workspace convention as project memory.");
        const result = setup.session.messages.find((message) => message.role === "toolResult");
        const objectId = /"objectId":"(mem_[0-9a-f]{32})"/.exec(textOf(result))?.[1];
        if (!objectId) throw new Error("eidnara_memory create returned no object id");
        return objectId;
    } finally {
        await setup.dispose();
    }
}

describe.skipIf(!active)("pi knowledge delivery under native authority", () => {
    let fixtureBin: string;
    const roots: string[] = [];

    beforeAll(async () => {
        fixtureBin = await buildDirectHostFixture();
    }, 600_000);

    afterAll(() => {
        for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    });

    it("serves stored project memory once per request across turns and a Pi compaction", async () => {
        const root = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-pi-delivery-")));
        roots.push(root);
        const dataDir = join(root, "data");
        mkdirSync(dataDir, { recursive: true });
        const { agent, extension } = await pluginModules();
        const saved = process.env.XDG_CONFIG_HOME;
        const stack = await HermeticHostStack.start({
            dataDir,
            fixtureBin,
            startTimeoutMs: 120_000,
            daemonConfig: {
                history_summarizer: { module_model: "fixture/deterministic" },
                ...COMPACTION_OFF,
                memory: { auto_capture: false },
            },
        });
        try {
            const project = join(root, "project");
            mkdirSync(project, { recursive: true });
            const configHome = join(root, "config");
            userConfig(configHome, stack.connectionFile, COMPACTION_OFF);
            process.env.XDG_CONFIG_HOME = configHome;
            const open = (options: Record<string, unknown>): Promise<PiHarness> => {
                extension.__test.clearPiEidnaraActive();
                return agent.createTestAgentSession({
                    cwd: project,
                    extensionFactories: [extension.default],
                    contextWindow: 1_000_000,
                    ...options,
                });
            };
            await awaitStore(stack, project);

            const anchor = await createdMemory(open);
            const first = memoryId("pi-delivery-first");
            const admitted = await stack.memorySeed(anchor, first, FIRST.category, FIRST.text);
            expect([admitted.effectiveMaturity, admitted.visibility]).toEqual([
                "verified",
                "automatic",
            ]);

            const h = await open({
                settings: { compaction: { enabled: false, keepRecentTokens: 1 } },
            });
            try {
                const prompts: string[] = [];
                let compactionRequests = 0;
                const turn = async (prompt: string, lines: string[]) => {
                    await h.session.prompt(prompt);
                    prompts.push(prompt);
                    expect(h.requests).toHaveLength(prompts.length + compactionRequests);
                    const request = h.requests.at(-1);
                    const served = (request?.messages ?? []).map(textOf);
                    expect(served[0]).toContain("<session-history></session-history>");
                    expect(occurrences(served[0] ?? "", PROJECT_MEMORY)).toBe(1);
                    expect(occurrences(JSON.stringify(request?.messages), PROJECT_MEMORY)).toBe(1);
                    for (const line of lines) {
                        expect(served[0]).toContain(line);
                        expect(occurrences(JSON.stringify(request), line)).toBe(1);
                    }
                    expect(served.at(-1)).toBe(prompt);
                    return served;
                };
                const firstLine = `${first}: ${FIRST.text}`;
                await turn("turn one", [firstLine]);
                await turn("turn two", [firstLine]);

                const second = memoryId("pi-delivery-second");
                await stack.memorySeed(anchor, second, SECOND.category, SECOND.text);
                const secondLine = `${second}: ${SECOND.text}`;
                const beforeCompaction = await turn("turn three", [firstLine, secondLine]);
                for (const prompt of prompts)
                    expect(beforeCompaction.filter((text) => text === prompt)).toHaveLength(1);

                const requestsBefore = h.requests.length;
                const outcome = await h.session.compact().then(
                    () => "compacted",
                    (error: Error) => error.message,
                );
                expect(outcome).toBe("compacted");
                compactionRequests = h.requests.length - requestsBefore;
                expect(compactionRequests).toBeGreaterThan(0);
                for (const request of h.requests.slice(requestsBefore)) {
                    const whole = JSON.stringify(request);
                    expect(whole).not.toContain(PROJECT_MEMORY);
                    expect(whole).not.toContain("<session-history>");
                }

                const afterCompaction = await turn("turn four", [firstLine, secondLine]);
                expect(h.session.messages[0]?.role).toBe("compactionSummary");
                const summary = h.sessionManager
                    .getEntries()
                    .find((entry) => entry.type === "compaction")?.summary;
                expect(summary).toBeTruthy();
                expect(afterCompaction.some((text) => text.includes(summary ?? ""))).toBe(true);

                expect(await stack.historySummarizerLive()).toBe(false);
                expect((await stack.backendCounters()).started).toBe(0);
            } finally {
                await h.dispose();
            }
        } finally {
            if (saved === undefined) delete process.env.XDG_CONFIG_HOME;
            else process.env.XDG_CONFIG_HOME = saved;
            await stack.stop();
        }
    }, 300_000);
});

describe.skipIf(active || !PI_REQUIRED)("pi knowledge delivery prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(rust.skipReason ?? pi.skipReason ?? "present").toBe("present");
    });
});
