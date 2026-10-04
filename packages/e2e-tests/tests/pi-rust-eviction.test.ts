import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
    type Binaries,
    buildBinaries,
    COVERED,
    openPiSession,
    pluginModules,
    segmentRanges,
    startFixture as startSeededFixture,
    textOf,
    userConfig,
} from "../src/pi-runner/rust-fixture";
import { detectPiPrereqs, PI_PLUGIN_ROOT, REPO_ROOT } from "../src/pi-runner/spawn";
import { detectRustModePrereqs } from "../src/rust-runner/hermetic-host";
import { writePiTier } from "../src/scale-report/pi-tier";

type Json = Record<string, unknown>;
type Entry = { id: string; type: string; summary?: string; firstKeptEntryId?: string };

const rust = detectRustModePrereqs();
const pi = detectPiPrereqs();
const active = rust.ok && pi.ok;
const COLD_MESSAGES = 1_000_000;
// Pi refuses a compaction that keeps fewer tokens than `keepRecentTokens` since the last kept
// entry; these transcripts are short.
const EVICTING = { compaction: { enabled: true, keepRecentTokens: 1 } };
// Pi refuses every compaction that keeps fewer tokens than these transcripts hold.
const NEVER_EVICTING = { compaction: { enabled: true, keepRecentTokens: 100_000_000 } };

interface Harness {
    session: {
        prompt(text: string): Promise<void>;
        navigateTree(id: string): Promise<unknown>;
    };
    sessionManager: { getEntries(): Entry[]; getBranch(): Entry[] };
    requests: { messages: unknown[] }[];
    dispose(): Promise<void>;
}

const allRanges = Array.from({ length: COVERED / 2 }, (_, k) => `${2 * k + 1}-${2 * k + 2}`);

function compactions(harness: Harness): Entry[] {
    return harness.sessionManager.getEntries().filter((entry) => entry.type === "compaction");
}

async function eventually(check: () => boolean, budgetMs = 10_000): Promise<void> {
    const deadline = Date.now() + budgetMs;
    while (!check()) {
        if (Date.now() > deadline) throw new Error(`condition did not hold within ${budgetMs} ms`);
        await Bun.sleep(10);
    }
}

async function recordTransforms() {
    const { transport } = await pluginModules();
    const calls: { body: Json; answer: Json }[] = [];
    const original = transport.HostModuleTransport.prototype.call;
    transport.HostModuleTransport.prototype.call = async function recorded(
        this: unknown,
        args: { method: string; body: Json },
    ) {
        const answer = await original.call(this, args);
        if (args.method === "transform") calls.push({ body: args.body, answer: answer as Json });
        return answer;
    };
    return {
        calls,
        restore() {
            transport.HostModuleTransport.prototype.call = original;
        },
    };
}

describe.skipIf(!active)("pi eviction and cold import against the direct-host fixture", () => {
    let binaries: Binaries;
    const roots: string[] = [];
    const startFixture = async (sessions: string[], root?: string) => {
        const fixture = await startSeededFixture(binaries, sessions, root);
        roots.push(fixture.root);
        return fixture;
    };

    beforeAll(async () => {
        binaries = await buildBinaries();
    }, 600_000);

    afterAll(() => {
        for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
    });

    it("evicts through the rendered boundary, keeps m0 bytes, and leaves m0 text when the daemon is down", async () => {
        const f = await startFixture(["pi-evict"]);
        const run = await openPiSession(
            f.root,
            f.stack,
            "pi-evict",
            join(f.root, "evict"),
            EVICTING,
        );
        const harness = run.harness as unknown as Harness;
        const recorded = await recordTransforms();
        try {
            await harness.session.prompt("first");
            const folded = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            await eventually(() => compactions(harness).length === 1);
            const [compaction] = compactions(harness);
            expect(compaction?.firstKeptEntryId).toBe(`m${COVERED}`);
            expect(compaction?.summary).toBe(folded[0] as string);

            await harness.session.prompt("second");
            const evicted = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            expect(evicted[0]).toBe(folded[0] as string);
            expect(evicted.filter((text) => text.includes(folded[0] as string))).toHaveLength(1);
            const sent = recorded.calls.at(-1)?.body.native_messages as { id: string }[];
            expect(sent[0]?.id).toBe(`m${COVERED}`);

            await f.stack.stop();
            await harness.session.prompt("with the daemon down");
            const down = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            // The failed pass leaves Pi's own array, which leads with its compaction summary.
            expect(down[0]).not.toBe(folded[0] as string);
            expect(down[0]).toStartWith("The conversation history before this point was compacted");
            expect(down[0]).toContain(folded[0] as string);
        } finally {
            recorded.restore();
            await run.close();
            await f.stack.stop().catch(() => undefined);
        }
    }, 300_000);

    it("serves the same m0 bytes when Pi refuses every eviction", async () => {
        const served: string[] = [];
        for (const settings of [EVICTING, NEVER_EVICTING]) {
            const f = await startFixture(["pi-evict"]);
            const run = await openPiSession(
                f.root,
                f.stack,
                "pi-evict",
                join(f.root, "evict"),
                settings,
            );
            const harness = run.harness as unknown as Harness;
            try {
                await harness.session.prompt("first");
                await Bun.sleep(50);
                await harness.session.prompt("second");
                served.push(textOf(harness.requests.at(-1)?.messages[0]));
                expect(compactions(harness)).toHaveLength(settings === EVICTING ? 1 : 0);
            } finally {
                await run.close();
                await f.stack.stop();
            }
        }
        expect(served[0]).toBe(served[1] as string);
        expect(segmentRanges(served[0] as string)).toEqual(allRanges);
    }, 300_000);

    it("keeps every segment and m0 bytes across a plugin restart and navigation after the compaction", async () => {
        const f = await startFixture(["pi-evict-restart"]);
        const project = join(f.root, "evict-restart");
        const run = await openPiSession(f.root, f.stack, "pi-evict-restart", project, EVICTING);
        const harness = run.harness as unknown as Harness;
        const { agent, extension } = await pluginModules();
        try {
            await harness.session.prompt("first");
            const m0 = textOf(harness.requests.at(-1)?.messages[0]);
            await eventually(() => compactions(harness).length === 1);
            await harness.session.prompt("second");

            const sessionManager = (run.harness as unknown as { sessionManager: unknown })
                .sessionManager;
            await harness.dispose();
            extension.__test.clearPiEidnaraActive();
            const recorded = await recordTransforms();
            const restarted = (await agent.createTestAgentSession({
                cwd: project,
                extensionFactories: [extension.default],
                sessionManager,
                contextWindow: 1_000_000,
                settings: EVICTING,
            })) as Harness;
            try {
                await restarted.session.prompt("after the restart");
                const served = (restarted.requests.at(-1)?.messages ?? []).map(textOf);
                expect(served[0]).toBe(m0);
                expect(segmentRanges(served[0] as string)).toEqual(allRanges);
                const rows = recorded.calls.at(-1)?.body.native_messages as { id: string }[];
                expect(rows[0]?.id).toBe(`m${COVERED}`);

                // The entry after the compaction keeps the compaction on the branch.
                const branch = restarted.sessionManager.getBranch();
                const at = branch.findIndex((entry) => entry.type === "compaction");
                const after = branch[at + 1]?.id as string;
                await restarted.session.navigateTree(after);
                await restarted.session.prompt("after navigating past the compaction");
                const navigated = (restarted.requests.at(-1)?.messages ?? []).map(textOf);
                expect(navigated[0]).toBe(m0);
                expect(segmentRanges(navigated[0] as string)).toEqual(allRanges);
                expect(recorded.calls.every((call) => call.answer.status === "ok")).toBe(true);
            } finally {
                recorded.restore();
                await restarted.dispose();
            }
        } finally {
            await run.close().catch(() => undefined);
            await f.stack.stop();
        }
    }, 300_000);

    it("sends the compaction summary once as ordinary content after a store reset", async () => {
        const f = await startFixture(["pi-reset"]);
        const project = join(f.root, "reset");
        const run = await openPiSession(f.root, f.stack, "pi-reset", project, EVICTING);
        const harness = run.harness as unknown as Harness;
        const { agent, extension } = await pluginModules();
        try {
            await harness.session.prompt("first");
            const folded = textOf(harness.requests.at(-1)?.messages[0]);
            await eventually(() => compactions(harness).length === 1);

            await f.stack.stop();
            const fresh = await startFixture([], f.root);
            userConfig(join(f.root, "pi-reset-config"), fresh.stack.connectionFile);
            await harness.dispose();
            extension.__test.clearPiEidnaraActive();
            const sessionManager = (run.harness as unknown as { sessionManager: unknown })
                .sessionManager;
            const recorded = await recordTransforms();
            const reset = (await agent.createTestAgentSession({
                cwd: project,
                extensionFactories: [extension.default],
                sessionManager,
                contextWindow: 1_000_000,
                settings: EVICTING,
            })) as Harness;
            try {
                await reset.session.prompt("after the reset");
                const sent = recorded.calls[0]?.body;
                expect(sent?.boundary).toBeNull();
                const rows = sent?.native_messages as { id: string }[];
                expect(rows[0]?.id).toMatch(/^eidnara:compactionSummary:/);
                expect(recorded.calls[0]?.answer.status).toBe("ok");
                const served = (reset.requests.at(-1)?.messages ?? []).map(textOf);
                expect(served.filter((text) => text.includes(folded))).toHaveLength(1);
            } finally {
                recorded.restore();
                await reset.dispose();
                await fresh.stack.stop();
            }
        } finally {
            await run.close().catch(() => undefined);
            await f.stack.stop().catch(() => undefined);
        }
    }, 300_000);

    it("restores full history when navigation leaves the compaction behind, and refolds keeping the prefix", async () => {
        const f = await startFixture(["pi-evict-nav"]);
        const run = await openPiSession(
            f.root,
            f.stack,
            "pi-evict-nav",
            join(f.root, "nav"),
            EVICTING,
        );
        const harness = run.harness as unknown as Harness;
        const recorded = await recordTransforms();
        try {
            await harness.session.prompt("first");
            await eventually(() => compactions(harness).length === 1);
            await harness.session.navigateTree("m30");
            const branch = harness.sessionManager.getBranch();
            expect(branch.some((entry) => entry.type === "compaction")).toBe(false);
            const before = recorded.calls.length;
            await harness.session.prompt("before the compaction");
            // Pi's array is the whole branch again, with no compaction summary in it.
            const restored = recorded.calls[before]?.body.native_messages as { id: string }[];
            expect(restored.some((row) => row.id.includes("compactionSummary"))).toBe(false);
            await harness.session.prompt("again");
            const refolded = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            expect(segmentRanges(refolded[0] as string)).toEqual(allRanges.slice(0, 15));
            // No pass resolves to the no-anchor reset: each renders a boundary.
            for (const call of recorded.calls) {
                expect(call.answer.status).toBe("ok");
                expect(call.answer.boundary).not.toBeNull();
            }
        } finally {
            recorded.restore();
            await run.close();
            await f.stack.stop();
        }
    }, 300_000);

    it("manages a cold 1M-message Pi session on its first pass with one half-cap upload", async () => {
        const f = await startFixture([]);
        const project = join(f.root, "cold");
        mkdirSync(project, { recursive: true });
        const { agent, extension } = await pluginModules();
        const file = join(f.root, "cold.jsonl");
        writePiTier(file, { messages: COLD_MESSAGES, window: 300, seed: 851, cwd: project });
        const configHome = join(f.root, "cold-config");
        userConfig(configHome, f.stack.connectionFile);
        const saved = process.env.XDG_CONFIG_HOME;
        process.env.XDG_CONFIG_HOME = configHome;
        extension.__test.clearPiEidnaraActive();
        const recorded = await recordTransforms();
        const harness = (await agent.createTestAgentSession({
            cwd: project,
            extensionFactories: [extension.default],
            sessionManager: agent.openSessionFile(file, project),
            contextWindow: 1_000_000,
        })) as Harness;
        const { piRowSize } = await import(join(PI_PLUGIN_ROOT, "src/transform/pi-ck.ts"));
        const { HALF_CAP_BLOCKS, HALF_CAP_BYTES } = await import(
            join(REPO_ROOT, "packages/opencode-plugin/src/hooks/context/window-cap.ts")
        );
        const sizeOf = (rows: unknown[]) =>
            rows.reduce<{ blocks: number; bytes: number }>(
                (sum, row) => {
                    const size = piRowSize(row);
                    return { blocks: sum.blocks + size.blocks, bytes: sum.bytes + size.bytes };
                },
                { blocks: 0, bytes: 0 },
            );
        try {
            await harness.session.prompt("cold");
            expect(recorded.calls).toHaveLength(1);
            const [first] = recorded.calls;
            expect(first?.body.boundary).toBeNull();
            const rows = first?.body.native_messages as { id: string }[];
            // The suffix is the contiguous newest run of the session, ending at the prompt.
            const branchIds = harness.sessionManager
                .getBranch()
                .filter((entry) => entry.type === "message")
                .map((entry) => entry.id);
            const promptAt = branchIds.indexOf(rows.at(-1)?.id as string);
            expect(promptAt).toBeGreaterThan(branchIds.length - 4);
            expect(rows.map((row) => row.id)).toEqual(
                branchIds.slice(promptAt - rows.length + 1, promptAt + 1),
            );
            const size = sizeOf(rows);
            expect(size.blocks).toBeLessThanOrEqual(HALF_CAP_BLOCKS);
            expect(size.bytes).toBeLessThanOrEqual(HALF_CAP_BYTES);
            expect(rows.length).toBeLessThan(COLD_MESSAGES / 1_000);
            expect(first?.answer.status).toBe("ok");
            // A suffix short of half the cap reaches it with the turn's new messages; the pass
            // that reaches it, still headed by the pinned message, is the cold import.
            const firstReason = (first?.answer.history_summarizer as Json | undefined)?.reason;
            expect(firstReason === "cold_import").toBe(size.blocks === HALF_CAP_BLOCKS);
            if (size.blocks < HALF_CAP_BLOCKS) {
                await harness.session.prompt("cold again");
                const second = recorded.calls[1];
                const secondRows = second?.body.native_messages as { id: string }[];
                expect(second?.body.boundary).toBeNull();
                expect(secondRows[0]?.id).toBe(rows[0]?.id);
                expect(sizeOf(secondRows).blocks).toBeGreaterThanOrEqual(HALF_CAP_BLOCKS);
                expect((second?.answer.history_summarizer as Json).reason).toBe("cold_import");
            }
        } finally {
            recorded.restore();
            await harness.dispose();
            if (saved === undefined) delete process.env.XDG_CONFIG_HOME;
            else process.env.XDG_CONFIG_HOME = saved;
            await f.stack.stop();
        }
    }, 600_000);

    it("manages a cold 1M-message OpenCode session on its first pass with one half-cap upload", async () => {
        const f = await startFixture([]);
        const project = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-cold-oc-")));
        roots.push(project);
        const { adapter, transport, maps } = await pluginModules();
        const calls: { body: Json; answer: Json }[] = [];
        const client = transport.createHostModuleClient(f.stack.connectionFile);
        const recorded = {
            call: async (args: { method: string; body: Json }) => {
                const answer = (await client.call(args)) as Json;
                if (args.method === "transform") calls.push({ body: args.body, answer });
                return answer;
            },
        };
        const openCode = adapter.createRustModeTransform(
            {
                client: {
                    app: { agents: async () => ({ data: [] }) },
                    session: { get: async () => ({ data: { directory: project } }) },
                },
                contextUsageMap: new maps.BoundedSessionMap(8),
                clearReasoningAge: 50,
                cacheTtl: "5m",
                directory: project,
                sessionDirectoryBySession: new Map(),
                isSubagentSession: () => false,
                systemPromptHashFor: () => "",
            },
            { moduleClient: recorded, projectRoot: project },
        );
        const messages = Array.from({ length: COLD_MESSAGES }, (_, index) => {
            const n = index + 1;
            const user = n % 2 === 1;
            return {
                info: {
                    id: `m${n}`,
                    sessionID: "oc-cold",
                    role: user ? "user" : "assistant",
                    time: { created: 1_700_000_000_000 + n },
                    ...(user ? {} : { providerID: "anthropic", modelID: "claude-sonnet-4-5" }),
                },
                parts: [{ id: `p${n}`, type: "text", text: `message ${n}` }],
            };
        });
        try {
            await openCode.run("oc-cold", { messages });
            expect(calls).toHaveLength(1);
            const [pass] = calls;
            expect(pass?.body.boundary).toBeNull();
            // Each message is one text block, so the suffix is exactly half the cap.
            const rows = pass?.body.native_messages as { info: { id: string } }[];
            expect(rows).toHaveLength(400);
            expect(rows[0]?.info.id).toBe(`m${COLD_MESSAGES - 399}`);
            expect((pass?.answer.history_summarizer as Json).reason).toBe("cold_import");
        } finally {
            client.disconnect();
            await f.stack.stop();
        }
    }, 600_000);
});
