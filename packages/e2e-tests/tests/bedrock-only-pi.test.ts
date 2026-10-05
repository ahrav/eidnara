import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CAPTURED_FACT, RESEARCHER_ANSWER } from "../src/bedrock-peer/answers";
import { classifyMemories } from "../src/bedrock-peer/classify";
import {
    detectHarnessRuntimeSources,
    ensurePiInstall,
    writeHarnessRuntime,
} from "../src/bedrock-peer/harness-runtime";
import {
    AWS_ENV,
    auditPeers,
    CREDENTIALS,
    completedWrapupRounds,
    isolateProviderEnv,
    isWrapupResult,
    MODEL,
    MODEL_REF,
    type Peers,
    peerDiagnostics,
    REQUIRED,
    served,
    startPeers,
    stopPeers,
    waitFor,
} from "../src/bedrock-peer/scenario";
import { PiRpcClient, type PiRpcEvent } from "../src/pi-runner/rpc-client";
import { createPiIsolatedEnv, detectPiPrereqs, type PiIsolatedEnv } from "../src/pi-runner/spawn";
import { buildDirectHostFixture, HermeticHostStack } from "../src/rust-runner/hermetic-host";
import { rustPrereqs } from "../src/rust-scenario-support";

const sources = detectHarnessRuntimeSources();
const piPrereqs = detectPiPrereqs();
const active = rustPrereqs.ok && piPrereqs.ok && sources.ok;
const WRAPUP_MARKER = "/eidnara-wrapup: ";

function texts(value: unknown): string[] {
    if (typeof value === "string") return [value];
    if (Array.isArray(value)) return value.flatMap(texts);
    if (value && typeof value === "object") return Object.values(value).flatMap(texts);
    return [];
}

describe.skipIf(!active)("bedrock-only callers: Pi", () => {
    let peers: Peers;
    let env: PiIsolatedEnv;
    let host: HermeticHostStack;
    let rpc: PiRpcClient;
    let runtimeDir: string;
    let dataDir: string;
    let sessionId: string;
    let restoreEnv: () => void = () => undefined;

    const diagnostics = (): string =>
        `${peerDiagnostics(peers)}\npi stderr:\n${rpc?.getStderr().slice(-3_000)}\nhost log:\n${host?.hostLog().slice(-6_000)}`;
    const identity = () => ({ project_root: env.workdir, harness: "pi", session: sessionId });

    async function prompt(text: string, timeoutMs = 180_000): Promise<PiRpcEvent> {
        const end = rpc.waitForEvent((event) => event.type === "agent_end", {
            timeoutMs,
            label: "agent_end",
        });
        const response = await rpc.sendCommand("prompt", { message: text }, { timeoutMs });
        if (response.success === false)
            throw new Error(`prompt failed: ${JSON.stringify(response)}\n${diagnostics()}`);
        return end;
    }

    async function messages(): Promise<string[]> {
        const response = await rpc.sendCommand<{ messages?: unknown[] }>("get_messages");
        return texts(response.data?.messages ?? []);
    }

    async function sessionStatus(): Promise<Record<string, unknown>> {
        return host.contextRequest(identity(), {
            method: "session.status",
            v: 1,
            session_id: sessionId,
        });
    }

    function wrapupResults(): string[] {
        const log = join(env.baseDir, "eidnara.log");
        if (!existsSync(log)) return [];
        return readFileSync(log, "utf8")
            .split(WRAPUP_MARKER)
            .slice(1)
            .map((entry) => (entry.split("\n", 1)[0] as string).replaceAll("\\n", "\n"));
    }

    beforeAll(async () => {
        if (!sources.ok) return;
        restoreEnv = isolateProviderEnv();
        peers = await startPeers();
        runtimeDir = mkdtempSync(join(tmpdir(), "eidnara-bedrock-only-pi-"));
        dataDir = mkdtempSync(join(tmpdir(), "eidnara-bedrock-only-pi-data-"));
        const harnessRuntime = writeHarnessRuntime({
            dir: runtimeDir,
            sources: sources.sources,
            piInstall: ensurePiInstall(),
            opencodeBaseUrl: peers.closure.http1Url,
            piBaseUrl: peers.closure.h2Url,
            anthropicSentinelUrl: peers.sentinel.http1Url,
            credentials: CREDENTIALS,
        });
        const fixtureBin = await buildDirectHostFixture();
        host = await HermeticHostStack.start({
            dataDir,
            fixtureBin,
            startTimeoutMs: 180_000,
            harnessRuntime,
            credentialSource: AWS_ENV,
            daemonConfig: { history_summarizer: { module_model: MODEL_REF } },
        });
        env = createPiIsolatedEnv();
        rpc = new PiRpcClient({
            mockProviderURL: peers.sentinel.http1Url,
            env,
            modelContextLimit: 30_000,
            bedrock: {
                baseUrl: peers.live.h2Url,
                model: MODEL,
                env: AWS_ENV,
                anthropicSentinelUrl: peers.sentinel.http1Url,
            },
            eidnaraConfig: {
                host: { connection_file: host.connectionFile },
                execute_threshold_percentage: 25,
                history_summarizer: { model: MODEL_REF },
                context_researcher: { model: MODEL_REF },
                memory: {
                    enabled: true,
                    auto_capture: true,
                    auto_search: { enabled: false },
                    git_commit_indexing: { enabled: false },
                },
            },
        });
        await rpc.start();
        const state = await rpc.sendCommand<{ sessionId?: string }>("get_state");
        if (typeof state.data?.sessionId !== "string")
            throw new Error(`no Pi session\n${diagnostics()}`);
        sessionId = state.data.sessionId;
    }, 600_000);

    afterAll(async () => {
        await rpc?.shutdown().catch(() => undefined);
        await host?.stop().catch(() => undefined);
        await stopPeers(peers);
        for (const dir of [runtimeDir, dataDir, env?.baseDir]) {
            if (dir) rmSync(dir, { recursive: true, force: true });
        }
        restoreEnv();
    });

    it("captures a memory through the harness's own Bedrock auth", async () => {
        await prompt(`Please note for this project: ${CAPTURED_FACT}`);
        const status = await waitFor(
            "a completed capture source",
            async () => {
                const status = await host.contextRequest(identity(), {
                    method: "memory.capture.status",
                    v: 2,
                    session_id: sessionId,
                });
                return typeof status.completed === "number" && status.completed >= 1
                    ? status
                    : undefined;
            },
            120_000,
            diagnostics,
        );
        expect(status.failed).toBe(0);
        expect(served(peers.live, "memory_capture")).toBeGreaterThanOrEqual(1);
    }, 180_000);

    it("augments a prompt through the context researcher", async () => {
        await prompt("/eidnara-aug Which port does the build listen on?");
        const augmented = await waitFor(
            "an augmented user message",
            async () =>
                (await messages()).find((text) =>
                    text.includes("<context_researcher-augmentation>"),
                ),
            120_000,
            diagnostics,
        );
        expect(augmented).toContain(RESEARCHER_ANSWER);
        expect(served(peers.live, "context_researcher")).toBeGreaterThanOrEqual(1);
    }, 180_000);

    it("classifies the captured memory through the Pi ModelExecution closure", async () => {
        const { objectIds, response } = await classifyMemories(host, identity(), MODEL_REF);
        expect(objectIds.length).toBeGreaterThanOrEqual(1);
        expect(response.classified).toBe(objectIds.length);
        expect(served(peers.closure, "memory_classifier")).toBeGreaterThanOrEqual(1);
    }, 300_000);

    it("fires the History Summarizer through the Pi ModelExecution closure", async () => {
        peers.live.conversationInputTokens = 3_000;
        for (let turn = 1; turn <= 10; turn += 1) {
            await prompt(`history turn ${turn}: ${"ballast words ".repeat(400)}`);
        }
        peers.live.conversationInputTokens = 27_000;
        await prompt(`history trigger: ${"ballast words ".repeat(400)}`);
        peers.live.conversationInputTokens = 500;
        await prompt("history follow-up");
        const status = await waitFor(
            "a published history segment",
            async () => {
                const status = await sessionStatus();
                return Number(status.history_segment_count ?? 0) > 0 ? status : undefined;
            },
            240_000,
            diagnostics,
        );
        expect(
            (status.history_summarizer as Record<string, unknown> | undefined)?.last_failure ??
                null,
        ).toBeNull();
        expect(served(peers.closure, "history_summarizer")).toBeGreaterThanOrEqual(1);
    }, 600_000);

    it("wraps up the session through the same closure", async () => {
        await prompt(`wrapup ballast: ${"ballast words ".repeat(400)}`);
        const before = Number((await sessionStatus()).history_segment_count ?? 0);
        let rounds: number | undefined;
        const results: string[] = [];
        for (let attempt = 1; rounds === undefined && attempt <= 3; attempt += 1) {
            const seen = wrapupResults().filter(isWrapupResult).length;
            await rpc.sendCommand(
                "prompt",
                { message: "/eidnara-wrapup 4" },
                { timeoutMs: 300_000 },
            );
            const result = await waitFor(
                "a wrapup result",
                () => wrapupResults().filter(isWrapupResult)[seen],
                300_000,
                diagnostics,
            );
            // A retryable result asks for another run after the next message, as the command tells the user.
            results.push(result);
            rounds = completedWrapupRounds(result);
            if (rounds === undefined) await prompt(`wrapup retry ${attempt}`);
            if (rounds === undefined && !result.startsWith("## Eidnara Wrapup — Partial")) {
                throw new Error(
                    `wrapup did not complete: ${result.slice(0, 600)}\n${diagnostics()}`,
                );
            }
        }
        expect(rounds ?? 0, results.join("\n---\n")).toBeGreaterThanOrEqual(1);
        expect(Number((await sessionStatus()).history_segment_count ?? 0)).toBeGreaterThan(before);
    }, 420_000);

    it("reached only the Bedrock peers, signed with the Bedrock row", () => {
        auditPeers(peers, { transport: "h2", userAgent: "aws-sdk-js/" });
    });
});

describe.skipIf(active || !REQUIRED)("bedrock-only callers: Pi prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(
            active,
            `${rustPrereqs.skipReason ?? ""} ${piPrereqs.skipReason ?? ""} ${sources.ok ? "" : sources.reason}`,
        ).toBe(true);
    });
});
