import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
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
import {
    createIsolatedEnv,
    type IsolatedEnv,
    type SpawnedOpencode,
    spawnOpencode,
} from "../src/opencode-runner/spawn";
import { buildDirectHostFixture, HermeticHostStack } from "../src/rust-runner/hermetic-host";
import { rustPrereqs } from "../src/rust-scenario-support";

const sources = detectHarnessRuntimeSources();
const active = rustPrereqs.ok && sources.ok;

interface OpencodeClient {
    session: {
        create(args: unknown): Promise<{ data?: { id: string } }>;
        prompt(args: unknown): Promise<{ data?: unknown; error?: unknown }>;
        command(args: unknown): Promise<{ data?: unknown; error?: unknown }>;
        messages(args: unknown): Promise<{
            data?: Array<{ parts?: Array<{ text?: string }> }>;
        }>;
    };
}

describe.skipIf(!active)("bedrock-only callers: OpenCode", () => {
    let peers: Peers;
    let env: IsolatedEnv;
    let host: HermeticHostStack;
    let opencode: SpawnedOpencode;
    let client: OpencodeClient;
    let runtimeDir: string;
    let sessionId: string;
    let restoreEnv: () => void = () => undefined;

    const diagnostics = (): string =>
        `${peerDiagnostics(peers)}\nopencode stderr:\n${opencode?.stderr().slice(-3_000)}\nhost log:\n${host?.hostLog().slice(-6_000)}`;
    const identity = () => ({ project_root: env.workdir, harness: "opencode", session: sessionId });

    async function prompt(text: string): Promise<void> {
        const result = await client.session.prompt({
            path: { id: sessionId },
            body: {
                model: { providerID: "amazon-bedrock", modelID: MODEL },
                parts: [{ type: "text", text }],
            },
        });
        if (result.error)
            throw new Error(`prompt failed: ${JSON.stringify(result.error)}\n${diagnostics()}`);
    }

    async function command(name: string, args: string): Promise<void> {
        const result = await client.session.command({
            path: { id: sessionId },
            body: { command: name, arguments: args, model: MODEL_REF },
        });
        if (result.error)
            throw new Error(`/${name} failed: ${JSON.stringify(result.error)}\n${diagnostics()}`);
    }

    async function sessionTexts(): Promise<string[]> {
        const result = await client.session.messages({ path: { id: sessionId } });
        return (result.data ?? []).flatMap((message) =>
            (message.parts ?? []).flatMap((part) =>
                typeof part.text === "string" ? [part.text] : [],
            ),
        );
    }

    async function segmentCount(): Promise<number> {
        const status = await host.primaryStatus(sessionId, env.workdir, "session.status");
        return Number(status.history_segment_count ?? 0);
    }

    beforeAll(async () => {
        if (!sources.ok) return;
        restoreEnv = isolateProviderEnv();
        peers = await startPeers();
        runtimeDir = mkdtempSync(join(tmpdir(), "eidnara-bedrock-only-"));
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
        env = createIsolatedEnv();
        host = await HermeticHostStack.start({
            dataDir: env.dataDir,
            fixtureBin,
            startTimeoutMs: 180_000,
            harnessRuntime,
            credentialSource: AWS_ENV,
            daemonConfig: { history_summarizer: { module_model: MODEL_REF } },
        });
        opencode = await spawnOpencode({
            mockProviderURL: peers.sentinel.http1Url,
            existingEnv: env,
            userHostConnectionFile: host.connectionFile,
            modelContextLimit: 30_000,
            bedrock: {
                baseURL: peers.live.http1Url,
                region: CREDENTIALS.region,
                model: MODEL,
                anthropicSentinelURL: peers.sentinel.http1Url,
            },
            eidnaraConfig: {
                execute_threshold_percentage: 25,
                protected_tags: 1,
                history_summarizer: { model: MODEL_REF },
                context_researcher: { model: MODEL_REF },
                memory: { auto_capture: true },
            },
            extraEnv: AWS_ENV,
        });
        const sdk = await import("@opencode-ai/sdk");
        client = sdk.createOpencodeClient({ baseUrl: opencode.url }) as unknown as OpencodeClient;
        const created = await client.session.create({ query: { directory: env.workdir } });
        if (!created.data) throw new Error(`session.create failed\n${diagnostics()}`);
        sessionId = created.data.id;
    }, 600_000);

    afterAll(async () => {
        await opencode?.kill().catch(() => undefined);
        await host?.stop().catch(() => undefined);
        await stopPeers(peers);
        if (runtimeDir) rmSync(runtimeDir, { recursive: true, force: true });
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
        await command("eidnara-aug", "Which port does the build listen on?");
        const augmented = await waitFor(
            "an augmented user message",
            async () =>
                (await sessionTexts()).find((text) =>
                    text.includes("<context_researcher-augmentation>"),
                ),
            120_000,
            diagnostics,
        );
        expect(augmented).toContain(RESEARCHER_ANSWER);
        expect(served(peers.live, "context_researcher")).toBeGreaterThanOrEqual(1);
    }, 180_000);

    it("classifies the captured memory through the OpenCode ModelExecution closure", async () => {
        const { objectIds, response } = await classifyMemories(host, identity(), MODEL_REF);
        expect(objectIds.length).toBeGreaterThanOrEqual(1);
        expect(response.classified).toBe(objectIds.length);
        expect(served(peers.closure, "memory_classifier")).toBeGreaterThanOrEqual(1);
    }, 300_000);

    it("fires the History Summarizer through the OpenCode ModelExecution closure", async () => {
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
                const status = await host.primaryStatus(sessionId, env.workdir, "session.status");
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
        const before = await segmentCount();
        let rounds: number | undefined;
        const results: string[] = [];
        for (let attempt = 1; rounds === undefined && attempt <= 3; attempt += 1) {
            const seen = (await sessionTexts()).filter(isWrapupResult).length;
            await command("eidnara-wrapup", "4");
            const result = await waitFor(
                "a wrapup result",
                async () => (await sessionTexts()).filter(isWrapupResult)[seen],
                300_000,
                diagnostics,
            );
            // A retryable result asks for another run after the next message, as the command tells the user.
            results.push(result);
            rounds = completedWrapupRounds(result);
            if (rounds === undefined) await prompt(`wrapup retry ${attempt}`);
            if (rounds === undefined && !result.startsWith("## Eidnara Wrapup — Partial")) {
                throw new Error(`wrapup did not complete: ${result}\n${diagnostics()}`);
            }
        }
        expect(rounds ?? 0, results.join("\n---\n")).toBeGreaterThanOrEqual(1);
        expect(await segmentCount()).toBeGreaterThan(before);
    }, 420_000);

    it("reached only the Bedrock peers, signed with the Bedrock row", () => {
        auditPeers(peers, { transport: "http1", userAgent: "opencode/1.18.22" });
    });
});

describe.skipIf(active || !REQUIRED)("bedrock-only callers: OpenCode prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(active, `${rustPrereqs.skipReason ?? ""} ${sources.ok ? "" : sources.reason}`).toBe(
            true,
        );
    });
});
