import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { PiTestHarness } from "../src/pi-harness";
import {
    createPiIsolatedEnv,
    detectPiPrereqs,
    PI_PLUGIN_ROOT,
    REPO_ROOT,
} from "../src/pi-runner/spawn";
import { EVAL_RUNNER } from "../src/rust-runner/daemon-examples";
import {
    buildDaemonExample,
    buildDirectHostFixture,
    detectRustModePrereqs,
    HermeticHostStack,
} from "../src/rust-runner/hermetic-host";

type Json = Record<string, unknown>;

const rust = detectRustModePrereqs();
const pi = detectPiPrereqs();
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";
const active = rust.ok && pi.ok;

const COVERED = 40;
const MESSAGES = 50;
const BASE_MS = Date.UTC(2026, 0, 1);
const body = (n: number) => `message ${n} body`;

function textOf(message: unknown): string {
    const record = message as { content?: unknown; parts?: unknown };
    const content = record.content ?? record.parts;
    if (typeof content === "string") return content;
    return (content as { type?: string; text?: string }[])
        .filter((part) => part.type === "text")
        .map((part) => part.text ?? "")
        .join("");
}

function writeTextSession(path: string, sessionId: string, cwd: string, count: number): void {
    const lines = [
        JSON.stringify({
            type: "session",
            version: 3,
            id: sessionId,
            timestamp: new Date(BASE_MS).toISOString(),
            cwd,
        }),
    ];
    for (let n = 1; n <= count; n += 1) {
        const timestamp = BASE_MS + n * 1_000;
        const message =
            n % 2 === 1
                ? { role: "user", content: [{ type: "text", text: body(n) }], timestamp }
                : {
                      role: "assistant",
                      content: [{ type: "text", text: body(n) }],
                      api: "anthropic-messages",
                      provider: "anthropic",
                      model: "claude-sonnet-4-5",
                      usage: {
                          input: 0,
                          output: 0,
                          cacheRead: 0,
                          cacheWrite: 0,
                          totalTokens: 0,
                          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
                      },
                      stopReason: "stop",
                      timestamp,
                  };
        lines.push(
            JSON.stringify({
                type: "message",
                id: `m${n}`,
                parentId: n === 1 ? null : `m${n - 1}`,
                timestamp: new Date(timestamp).toISOString(),
                message,
            }),
        );
    }
    writeFileSync(path, `${lines.join("\n")}\n`);
}

function openCodeTranscript(sessionId: string, count: number): Json[] {
    return Array.from({ length: count }, (_, index) => {
        const n = index + 1;
        const user = n % 2 === 1;
        return {
            info: {
                id: `m${n}`,
                sessionID: sessionId,
                role: user ? "user" : "assistant",
                time: { created: BASE_MS + n * 1_000 },
                ...(user ? {} : { providerID: "anthropic", modelID: "claude-sonnet-4-5" }),
            },
            parts: [{ id: `p${n}`, type: "text", text: body(n) }],
        };
    });
}

interface Binaries {
    fixtureBin: string;
    evalRunner: string;
}

let binaries: Binaries | undefined;
const roots: string[] = [];

/** Seeds `sessions` into a fresh store, then starts the fixture over it. */
async function startFixture(
    sessions: string[],
): Promise<{ root: string; stack: HermeticHostStack }> {
    const { fixtureBin, evalRunner } = binaries as Binaries;
    const root = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-pi-rust-")));
    roots.push(root);
    const dataDir = join(root, "data");
    mkdirSync(dataDir, { recursive: true });
    for (const session of sessions) {
        const result = spawnSync(
            evalRunner,
            [
                "scale-seed",
                "--state-root",
                dataDir,
                "--session",
                session,
                "--segments",
                String(COVERED / 2),
            ],
            { encoding: "utf8" },
        );
        if (result.status !== 0) throw new Error(`scale-seed failed: ${result.stderr}`);
    }
    const stack = await HermeticHostStack.start({ dataDir, fixtureBin, startTimeoutMs: 120_000 });
    return { root, stack };
}

async function pluginModules() {
    const opencode = join(REPO_ROOT, "packages/opencode-plugin/src");
    const [adapter, transport, maps, agent, extension] = await Promise.all([
        import(join(opencode, "hooks/context/opencode-transform-adapter.ts")),
        import(join(opencode, "hooks/context/module-transport.ts")),
        import(join(opencode, "shared/bounded-session-map.ts")),
        import(join(PI_PLUGIN_ROOT, "src/__tests__/agent-session.ts")),
        import(join(PI_PLUGIN_ROOT, "src/index.ts")),
    ]);
    return { adapter, transport, maps, agent, extension };
}

function userConfig(configHome: string, connectionFile: string): void {
    mkdirSync(join(configHome, "eidnara"), { recursive: true });
    writeFileSync(
        join(configHome, "eidnara", "eidnara.jsonc"),
        JSON.stringify({
            history_summarizer: { model: "fixture/deterministic" },
            host: { connection_file: connectionFile },
            memory: { auto_search: { enabled: false } },
        }),
    );
}

interface PiRun {
    harness: {
        session: {
            prompt(text: string): Promise<void>;
            messages: unknown[];
            navigateTree(id: string): Promise<unknown>;
        };
        requests: { messages: unknown[] }[];
        dispose(): Promise<void>;
    };
    close(): Promise<void>;
}

/** Opens a Pi session over a fresh `m1`..`m50` transcript with the plugin dialing `stack`. */
async function openPiSession(
    root: string,
    stack: HermeticHostStack,
    sessionId: string,
    project: string,
): Promise<PiRun> {
    const modules = await pluginModules();
    const configHome = join(root, `${sessionId}-config`);
    mkdirSync(project, { recursive: true });
    userConfig(configHome, stack.connectionFile);
    const file = join(root, `${sessionId}.jsonl`);
    writeTextSession(file, sessionId, project, MESSAGES);
    const saved = process.env.XDG_CONFIG_HOME;
    process.env.XDG_CONFIG_HOME = configHome;
    modules.extension.__test.clearPiEidnaraActive();
    const harness = await modules.agent.createTestAgentSession({
        cwd: project,
        extensionFactories: [modules.extension.default],
        sessionManager: modules.agent.openSessionFile(file, project),
        contextWindow: 1_000_000,
    });
    return {
        harness,
        async close() {
            try {
                await harness.dispose();
            } finally {
                if (saved === undefined) delete process.env.XDG_CONFIG_HOME;
                else process.env.XDG_CONFIG_HOME = saved;
            }
        },
    };
}

const CROSS_ROOT =
    "this session's context lineage belongs to another project root; start a new session in this directory.";

function noteCall(
    client: { call(args: Record<string, unknown>): Promise<unknown> },
    sessionId: string,
    projectRoot: string,
): Promise<unknown> {
    return client.call({
        sessionId,
        projectRoot,
        method: "eidnara_note",
        body: { name: "eidnara_note", arguments: { action: "read" } },
    });
}

function untagged(text: string): string {
    return text.replace(/^(<!--[^>]*-->\n)?§\d+§ /, "");
}

/** The `## a-b` ranges an m0 text renders, oldest first. */
function segmentRanges(m0: string): string[] {
    return [...m0.matchAll(/^## (\d+-\d+) ·/gm)].map((match) => match[1] as string);
}

describe.skipIf(!active)("pi folding against the direct-host fixture", () => {
    beforeAll(async () => {
        const [fixtureBin, evalRunner] = await Promise.all([
            buildDirectHostFixture(),
            buildDaemonExample(EVAL_RUNNER),
        ]);
        binaries = { fixtureBin, evalRunner };
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
            await first.harness.dispose();
            await expect(noteCall(client, "pi-cd", rootB)).rejects.toThrow(CROSS_ROOT);

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
                expect(await noteCall(client, "pi-cd", rootB)).toBeDefined();
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
