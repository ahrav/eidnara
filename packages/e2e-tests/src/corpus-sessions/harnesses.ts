import { join } from "node:path";

import { awaitStore, pluginModules, userConfig } from "../pi-runner/rust-fixture.ts";
import { type HarnessHost, RustTestHarness } from "../rust-harness.ts";
import { buildDirectHostFixture, HermeticHostStack } from "../rust-runner/hermetic-host.ts";
import type { ExpectedRecord } from "../source-corpus/expected.ts";
import { openCodeMcpConfig } from "./opencode-mix.ts";
import { corpusProbeExtension, PI_MIX_TOOLS, type PiMixHarness } from "./pi-mix.ts";
import { prepareWorkspace } from "./workspace.ts";

const { mkdirSync, mkdtempSync, realpathSync, rmSync } = process.getBuiltinModule("node:fs");
const { tmpdir } = process.getBuiltinModule("node:os");

/** The daemon configuration every corpus session runs under; `capture` is the consent switch. */
export function corpusDaemonConfig(capture: boolean): Record<string, unknown> {
    return {
        history_summarizer: { module_model: "fixture/deterministic" },
        compaction: { enabled: false },
        memory: { enabled: true, auto_capture: false },
        source_capture: { enabled: capture },
    };
}

/** Every record except the session's final assistant reply, which no later pass offers. */
export function offered(records: readonly ExpectedRecord[]): ExpectedRecord[] {
    let last = -1;
    records.forEach((record, index) => {
        if (record.role === "assistant") last = index;
    });
    return records.filter((_, index) => index !== last);
}

export async function startOpenCode(capture: boolean): Promise<RustTestHarness<HermeticHostStack>> {
    return RustTestHarness.createWithHost(
        { modelContextLimit: 1_000_000, openCodeConfigExtra: openCodeMcpConfig() },
        async (env) =>
            HermeticHostStack.start({
                dataDir: env.dataDir,
                fixtureBin: await buildDirectHostFixture(),
                daemonConfig: corpusDaemonConfig(capture),
            }),
    );
}

/** An OpenCode harness and the workspace its session runs in. */
export async function openCodeWorkspace(h: RustTestHarness<HarnessHost>) {
    const root = prepareWorkspace(join(h.env.workdir, ".."));
    return { root, sessionId: await h.createSession(root) };
}

export interface PiCorpusRun {
    readonly stack: HermeticHostStack;
    readonly root: string;
    readonly project: string;
    readonly file: string;
    readonly harness: PiMixHarness & { dispose(): Promise<void> };
    close(): Promise<void>;
}

/** Starts the fixture and one in-process Pi session with the corpus tools, persisting its JSONL. */
export async function startPi(capture: boolean): Promise<PiCorpusRun> {
    const root = realpathSync(mkdtempSync(join(tmpdir(), "source-corpus-pi-")));
    const dataDir = join(root, "data");
    mkdirSync(dataDir);
    const saved = process.env.XDG_CONFIG_HOME;
    const restore = () => {
        if (saved === undefined) delete process.env.XDG_CONFIG_HOME;
        else process.env.XDG_CONFIG_HOME = saved;
    };
    let modules: Awaited<ReturnType<typeof pluginModules>>;
    let stack: HermeticHostStack;
    try {
        modules = await pluginModules();
        stack = await HermeticHostStack.start({
            dataDir,
            fixtureBin: await buildDirectHostFixture(),
            startTimeoutMs: 120_000,
            daemonConfig: corpusDaemonConfig(capture),
        });
    } catch (error) {
        rmSync(root, { recursive: true, force: true });
        throw error;
    }
    const { agent, extension } = modules;
    const teardown = async () => {
        restore();
        try {
            await stack.stop();
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    };
    try {
        const project = prepareWorkspace(root);
        const configHome = join(root, "config");
        userConfig(configHome, stack.connectionFile, { compaction: { enabled: false } });
        process.env.XDG_CONFIG_HOME = configHome;
        await awaitStore(stack, project);
        extension.__test.clearPiEidnaraActive();
        const file = join(root, "session.jsonl");
        const harness = await agent.createTestAgentSession({
            cwd: project,
            extensionFactories: [extension.default, corpusProbeExtension],
            contextWindow: 1_000_000,
            sessionManager: agent.openSessionFile(file, project),
            tools: PI_MIX_TOOLS,
        });
        return {
            stack,
            root,
            project,
            file,
            harness,
            async close() {
                try {
                    await harness.dispose();
                } finally {
                    await teardown();
                }
            },
        };
    } catch (error) {
        await teardown();
        throw error;
    }
}
