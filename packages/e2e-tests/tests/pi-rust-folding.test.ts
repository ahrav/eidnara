import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";

import { PiTestHarness } from "../src/pi-harness";
import {
    BASE_MS,
    type Binaries,
    body,
    buildBinaries,
    COVERED,
    CROSS_ROOT,
    MESSAGES,
    noteCall,
    openCodeTranscript,
    openPiSession,
    pluginModules,
    segmentRanges,
    startFixture as startSeededFixture,
    textOf,
    untagged,
} from "../src/pi-runner/rust-fixture";
import { createPiIsolatedEnv, detectPiPrereqs } from "../src/pi-runner/spawn";
import { detectRustModePrereqs } from "../src/rust-runner/hermetic-host";

const rust = detectRustModePrereqs();
const pi = detectPiPrereqs();
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";
const active = rust.ok && pi.ok;

describe.skipIf(!active)("pi folding against the direct-host fixture", () => {
    let binaries: Binaries;
    const roots: string[] = [];
    const startFixture = async (sessions: string[]) => {
        const fixture = await startSeededFixture(binaries, sessions);
        roots.push(fixture.root);
        return fixture;
    };

    beforeAll(async () => {
        binaries = await buildBinaries();
    }, 600_000);

    afterAll(() => {
        for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    });

    it("folds a Pi session to the m0, m1, and tail an equivalent OpenCode session gets", async () => {
        const f = await startFixture(["oc-parity", "pi-parity"]);
        const modules = await pluginModules();
        const project = join(f.root, "parity");
        const run = await openPiSession(f.root, f.stack, "pi-parity", project);
        const harness = run.harness;
        try {
            await harness.session.prompt(body(MESSAGES + 1));
            const served = (harness.requests[0]?.messages ?? []).map(textOf);
            const promptedAt = (harness.session.messages.at(-2) as { timestamp: number }).timestamp;
            const client = modules.transport.createHostModuleClient(f.stack.connectionFile);
            // Pi reads the previous response's completion from the window's last assistant
            // message; OpenCode reads it from its usage map, so the map carries the same time.
            const contextUsageMap = new modules.maps.BoundedSessionMap(8);
            contextUsageMap.set("oc-parity", {
                usage: { percentage: 0, inputTokens: 0 },
                updatedAt: BASE_MS + MESSAGES * 1_000,
                lastResponseTime: BASE_MS + MESSAGES * 1_000,
            });
            const openCode = modules.adapter.createRustModeTransform(
                {
                    client: {
                        app: { agents: async () => ({ data: [] }) },
                        session: { get: async () => ({ data: { directory: project } }) },
                    },
                    contextUsageMap,
                    clearReasoningAge: 50,
                    cacheTtl: "5m",
                    directory: project,
                    sessionDirectoryBySession: new Map(),
                    isSubagentSession: () => false,
                    systemPromptHashFor: () => "",
                },
                { moduleClient: client, projectRoot: project },
            );
            const messages = openCodeTranscript("oc-parity", MESSAGES + 1);
            // The prompt's own timestamp decides its temporal mark, so both transcripts share it.
            (messages.at(-1)?.info as { time: { created: number } }).time.created = promptedAt;
            const output = { messages };
            await openCode.run("oc-parity", output);
            client.disconnect();

            const expected = output.messages.map(textOf);
            expect(expected[0]).toContain("<session-history>");
            expect(expected.at(-1)).toMatch(/§\d+§ message 51 body$/);
            expect(expected[1]).toBe("(no new content since last materialization)");
            expect(expected).toHaveLength(MESSAGES - COVERED + 3);
            expect(served).toEqual(expected);
        } finally {
            await run.close();
            await f.stack.stop();
        }
    }, 300_000);

    it("re-walks to the same entry ids after a plugin restart and infers no revert", async () => {
        const f = await startFixture(["pi-restart"]);
        const project = join(f.root, "restart");
        const run = await openPiSession(f.root, f.stack, "pi-restart", project);
        const { agent, extension } = await pluginModules();
        const allRanges = Array.from(
            { length: COVERED / 2 },
            (_, k) => `${2 * k + 1}-${2 * k + 2}`,
        );
        try {
            await run.harness.session.prompt("before the restart");
            const sessionManager = (run.harness as unknown as { sessionManager: unknown })
                .sessionManager;
            await run.harness.dispose();
            extension.__test.clearPiEidnaraActive();
            const restarted = await agent.createTestAgentSession({
                cwd: project,
                extensionFactories: [extension.default],
                sessionManager,
                contextWindow: 1_000_000,
            });
            try {
                await restarted.session.prompt("after the restart");
                const served = (restarted.requests.at(-1)?.messages ?? []).map(textOf);
                expect(segmentRanges(served[0] as string)).toEqual(allRanges);
                expect(served.slice(2).map(untagged)).toEqual([
                    ...Array.from({ length: MESSAGES - COVERED }, (_, k) => body(COVERED + 1 + k)),
                    "before the restart",
                    "ok",
                    "after the restart",
                ]);
            } finally {
                await restarted.dispose();
            }
        } finally {
            await run.close().catch(() => undefined);
            await f.stack.stop();
        }
    }, 300_000);

    it("resolves navigation across the rendered boundary and keeps every surviving segment", async () => {
        const f = await startFixture(["pi-navigation"]);
        const run = await openPiSession(f.root, f.stack, "pi-navigation", join(f.root, "nav"));
        const { harness } = run;
        const allRanges = Array.from(
            { length: COVERED / 2 },
            (_, k) => `${2 * k + 1}-${2 * k + 2}`,
        );
        try {
            await harness.session.prompt("first");
            const first = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            expect(segmentRanges(first[0] as string)).toEqual(allRanges);

            // m46 follows the rendered boundary m40, which stays in view.
            await harness.session.navigateTree("m46");
            await harness.session.prompt("after the boundary");
            const after = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            expect(segmentRanges(after[0] as string)).toEqual(allRanges);
            expect(after.slice(2).map(untagged)).toEqual([
                ...Array.from({ length: 6 }, (_, k) => body(COVERED + 1 + k)),
                "after the boundary",
            ]);

            // m30 precedes the rendered boundary, so the boundary leaves the branch: the first
            // pass reconciles and still serves the new prompt, and the next folds from m30.
            await harness.session.navigateTree("m30");
            await harness.session.prompt("before the boundary");
            const reconciling = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            // The deferring pass keeps the rendered m0 until the next pass truncates and refolds.
            expect(segmentRanges(reconciling[0] as string)).toEqual(allRanges);
            expect(reconciling.slice(2).map(untagged)).toEqual(["before the boundary"]);
            await harness.session.prompt("again");
            const refolded = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            expect(segmentRanges(refolded[0] as string)).toEqual(allRanges.slice(0, 15));
            expect(refolded.slice(2).map(untagged)).toEqual(["before the boundary", "ok", "again"]);
        } finally {
            await run.close();
            await f.stack.stop();
        }
    }, 300_000);

    it("folds the Pi RPC process's context through the fixture and refuses a cross-root call", async () => {
        const f = await startFixture([]);
        const env = createPiIsolatedEnv();
        const other = join(env.baseDir, "other-project");
        mkdirSync(other, { recursive: true });
        const { transport } = await pluginModules();
        const client = transport.createHostModuleClient(f.stack.connectionFile);
        try {
            const harness = await PiTestHarness.create({
                env,
                eidnaraConfig: {
                    history_summarizer: { model: "fixture/deterministic" },
                    host: { connection_file: f.stack.connectionFile },
                },
            });
            let sessionId = "";
            try {
                const turn = await harness.sendPrompt("through the daemon", { timeoutMs: 120_000 });
                expect(turn.assistantText).toBeTruthy();
                sessionId = turn.sessionId as string;
            } finally {
                await harness.dispose();
            }
            const passes = readFileSync(join(env.baseDir, "eidnara.log"), "utf8")
                .split("\n")
                .filter((line) => line.includes(`[${sessionId}] rust pass:`));
            expect(passes.some((line) => line.includes("applied=true"))).toBe(true);
            await expect(noteCall(client, sessionId, other)).rejects.toThrow(CROSS_ROOT);
        } finally {
            client.disconnect();
            await f.stack.stop();
            rmSync(env.baseDir, { recursive: true, force: true });
        }
    }, 300_000);

    it("after a /cd, forms lineage on the new root at its first pass, before any tool call", async () => {
        const f = await startFixture([]);
        const { transport, agent, extension } = await pluginModules();
        const rootA = join(f.root, "project-a");
        const rootB = join(f.root, "project-b");
        mkdirSync(rootB, { recursive: true });
        const client = transport.createHostModuleClient(f.stack.connectionFile);
        const first = await openPiSession(f.root, f.stack, "pi-cd", rootA);
        const sessionManager = (first.harness as unknown as { sessionManager: unknown })
            .sessionManager;
        try {
            await first.harness.session.prompt("in project a");
            // Lineage formed on the session's own cwd, not the process's boot directory.
            expect(await noteCall(client, "pi-cd", rootA)).toBeDefined();
            await expect(noteCall(client, "pi-cd", rootB)).rejects.toThrow(CROSS_ROOT);
            await first.harness.dispose();

            // Pi continues a session in another directory through a runtime built for that cwd.

            extension.__test.clearPiEidnaraActive();
            const moved = await agent.createTestAgentSession({
                cwd: rootB,
                extensionFactories: [extension.default],
                sessionManager,
                contextWindow: 1_000_000,
            });
            try {
                await moved.session.prompt("in project b");
                expect(moved.requests).toHaveLength(1);
                // The first runtime's shutdown disconnected the shared transport.
                const after = transport.createHostModuleClient(f.stack.connectionFile);
                try {
                    expect(await noteCall(after, "pi-cd", rootB)).toBeDefined();
                } finally {
                    after.disconnect();
                }
            } finally {
                await moved.dispose();
            }
        } finally {
            await first.close().catch(() => undefined);
            client.disconnect();
            await f.stack.stop();
        }
    }, 300_000);
});

describe.skipIf(active || !PI_REQUIRED)("pi folding prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(active, `${rust.skipReason ?? ""} ${pi.skipReason ?? ""}`).toBe(true);
    });
});
