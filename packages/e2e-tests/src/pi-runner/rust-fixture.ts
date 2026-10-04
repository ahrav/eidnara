import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EVAL_RUNNER } from "../rust-runner/daemon-examples";
import {
    buildDaemonExample,
    buildDirectHostFixture,
    HermeticHostStack,
} from "../rust-runner/hermetic-host";
import { PI_PLUGIN_ROOT, REPO_ROOT } from "./spawn";

type Json = Record<string, unknown>;

export const COVERED = 40;
export const MESSAGES = 50;
export const BASE_MS = Date.UTC(2026, 0, 1);
export const body = (n: number) => `message ${n} body`;

export function textOf(message: unknown): string {
    const record = message as { content?: unknown; parts?: unknown };
    const content = record.content ?? record.parts;
    if (typeof content === "string") return content;
    return (content as { type?: string; text?: string }[])
        .filter((part) => part.type === "text")
        .map((part) => part.text ?? "")
        .join("");
}

export function writeTextSession(
    path: string,
    sessionId: string,
    cwd: string,
    count: number,
): void {
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

export function openCodeTranscript(sessionId: string, count: number): Json[] {
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

export interface Binaries {
    fixtureBin: string;
    evalRunner: string;
}

export function buildBinaries(): Promise<Binaries> {
    return Promise.all([buildDirectHostFixture(), buildDaemonExample(EVAL_RUNNER)]).then(
        ([fixtureBin, evalRunner]) => ({ fixtureBin, evalRunner }),
    );
}

/** Seeds `sessions` into a fresh store under `root`, then starts the fixture over it. */
export async function startFixture(
    binaries: Binaries,
    sessions: string[],
    root = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-pi-rust-"))),
): Promise<{ root: string; stack: HermeticHostStack }> {
    const { fixtureBin, evalRunner } = binaries;
    const dataDir = join(root, `data-${Date.now()}`);
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
    // The fixture opens its store after it reports ready; a pass sent before then declines.
    const deadline = Date.now() + 60_000;
    for (;;) {
        try {
            await stack.primaryStatus("store-probe", root, "session.status");
            break;
        } catch (error) {
            const code = (error as { code?: unknown }).code;
            if (code !== "store_unavailable" || Date.now() > deadline) throw error;
            await Bun.sleep(100);
        }
    }
    return { root, stack };
}

export async function pluginModules() {
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

export function userConfig(configHome: string, connectionFile: string): void {
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

export interface PiRun {
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
export async function openPiSession(
    root: string,
    stack: HermeticHostStack,
    sessionId: string,
    project: string,
    settings?: Record<string, unknown>,
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
        settings,
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

export const CROSS_ROOT =
    "this session's context lineage belongs to another project root; start a new session in this directory.";

export function noteCall(
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

export function untagged(text: string): string {
    return text.replace(/^(<!--[^>]*-->\n)?§\d+§ /, "");
}

/** The `## a-b` ranges an m0 text renders, oldest first. */
export function segmentRanges(m0: string): string[] {
    return [...m0.matchAll(/^## (\d+-\d+) ·/gm)].map((match) => match[1] as string);
}
