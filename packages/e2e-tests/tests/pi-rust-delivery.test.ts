import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { awaitStore, pluginModules, textOf, userConfig } from "../src/pi-runner/rust-fixture";
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

function memoryId(label: string): string {
    return `mem_${createHash("sha256").update(label).digest("hex").slice(0, 32)}`;
}

function occurrences(haystack: string, needle: string): number {
    return haystack.split(needle).length - 1;
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
        const project = join(root, "project");
        mkdirSync(project, { recursive: true });
        const configHome = join(root, "config");
        userConfig(configHome, stack.connectionFile, COMPACTION_OFF);
        const saved = process.env.XDG_CONFIG_HOME;
        process.env.XDG_CONFIG_HOME = configHome;
        const { agent, extension } = await pluginModules();
        const open = (options: Record<string, unknown> = {}) => {
            extension.__test.clearPiEidnaraActive();
            return agent.createTestAgentSession({
                cwd: project,
                extensionFactories: [extension.default],
                contextWindow: 1_000_000,
                ...options,
            });
        };
        try {
            await awaitStore(stack, project);

            const setup = await open({ tools: ["eidnara_memory"] });
            let anchor: string | undefined;
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
                const result = setup.session.messages.find(
                    (message: { role?: string }) => message.role === "toolResult",
                );
                anchor = /"objectId":"(mem_[0-9a-f]{32})"/.exec(textOf(result))?.[1];
            } finally {
                await setup.dispose();
            }
            if (!anchor) throw new Error("eidnara_memory create returned no object id");
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
                const expectDelivered = (lines: string[]) => {
                    const request = h.requests.at(-1);
                    const served = (request?.messages ?? []).map(textOf);
                    const whole = JSON.stringify(request?.messages ?? []);
                    expect(served[0]).toContain("<session-history></session-history>");
                    expect(occurrences(served[0] ?? "", PROJECT_MEMORY)).toBe(1);
                    expect(occurrences(whole, PROJECT_MEMORY)).toBe(1);
                    for (const line of lines) {
                        expect(served[0]).toContain(line);
                        expect(occurrences(whole, line)).toBe(1);
                    }
                    return served;
                };
                const firstLine = `${first}: ${FIRST.text}`;
                await h.session.prompt("turn one");
                expectDelivered([firstLine]);
                await h.session.prompt("turn two");
                expectDelivered([firstLine]);

                const second = memoryId("pi-delivery-second");
                await stack.memorySeed(anchor, second, SECOND.category, SECOND.text);
                const secondLine = `${second}: ${SECOND.text}`;
                await h.session.prompt("turn three");
                expectDelivered([firstLine, secondLine]);

                const beforeCompaction = h.requests.length;
                const outcome = await h.session.compact().then(
                    () => "compacted",
                    (error: Error) => error.message,
                );
                expect(outcome).toBe("compacted");
                const compactionRequests = h.requests.slice(beforeCompaction);
                expect(compactionRequests.length).toBeGreaterThan(0);
                for (const request of compactionRequests)
                    expect(JSON.stringify(request.messages)).not.toContain(PROJECT_MEMORY);

                await h.session.prompt("turn four");
                const served = expectDelivered([firstLine, secondLine]);
                expect((h.session.messages[0] as { role?: string }).role).toBe("compactionSummary");
                const compaction = h.sessionManager
                    .getEntries()
                    .find((entry: { type: string }) => entry.type === "compaction");
                expect(served[2]).toContain((compaction as { summary: string }).summary);

                expect(h.requests).toHaveLength(4 + compactionRequests.length);
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
