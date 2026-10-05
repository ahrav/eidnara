import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CAPTURED_FACT, type Caller, RESEARCHER_ANSWER } from "../src/bedrock-peer/answers";
import { classifyMemories } from "../src/bedrock-peer/classify";
import {
    detectHarnessRuntimeSources,
    ensurePiInstall,
    writeHarnessRuntime,
} from "../src/bedrock-peer/harness-runtime";
import { BedrockPeer } from "../src/bedrock-peer/server";
import {
    createIsolatedEnv,
    type IsolatedEnv,
    type SpawnedOpencode,
    spawnOpencode,
} from "../src/opencode-runner/spawn";
import { buildDirectHostFixture, HermeticHostStack } from "../src/rust-runner/hermetic-host";
import { rustPrereqs } from "../src/rust-scenario-support";

const MODEL = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
const MODEL_REF = `amazon-bedrock/${MODEL}`;
const CREDENTIALS = {
    accessKeyId: "AKIDBEDROCKONLYE2E",
    secretAccessKey: "bedrock/only+e2e/secret",
    sessionToken: "bedrock-only-e2e-session-token",
    region: "us-east-1",
};
const AWS_ENV = {
    AWS_ACCESS_KEY_ID: CREDENTIALS.accessKeyId,
    AWS_SECRET_ACCESS_KEY: CREDENTIALS.secretAccessKey,
    AWS_SESSION_TOKEN: CREDENTIALS.sessionToken,
    AWS_REGION: CREDENTIALS.region,
};
const sources = detectHarnessRuntimeSources();
const REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";

interface OpencodeClient {
    session: {
        create(args: unknown): Promise<{ data?: { id: string } }>;
        prompt(args: unknown): Promise<{ data?: unknown; error?: unknown }>;
        command(args: unknown): Promise<{ data?: unknown; error?: unknown }>;
        messages(args: unknown): Promise<{
            data?: Array<{
                info?: { role?: string };
                parts?: Array<{ type?: string; text?: string }>;
            }>;
        }>;
    };
}

async function waitFor<T>(
    what: string,
    probe: () => Promise<T | undefined> | T | undefined,
    timeoutMs: number,
    diagnostics: () => string,
): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        const value = await probe();
        if (value !== undefined) return value;
        await Bun.sleep(250);
    }
    throw new Error(`${what} did not happen within ${timeoutMs}ms\n${diagnostics()}`);
}

describe.skipIf(!rustPrereqs.ok || !sources.ok)("bedrock-only callers: OpenCode", () => {
    let peer: BedrockPeer;
    let env: IsolatedEnv;
    let host: HermeticHostStack;
    let opencode: SpawnedOpencode;
    let client: OpencodeClient;
    let runtimeDir: string;
    let sessionId: string;

    const diagnostics = (): string =>
        `peer: ${JSON.stringify(peer.requests.map(({ caller, status, transport, userAgent }) => ({ caller, status, transport, userAgent })))}\n` +
        `opencode stderr:\n${opencode?.stderr().slice(-3_000)}\nhost log:\n${host?.hostLog().slice(-6_000)}`;

    const served = (caller: Caller): number =>
        peer.callers().filter((served) => served === caller).length;

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

    beforeAll(async () => {
        if (!sources.ok) return;
        for (const name of Object.keys(process.env)) {
            if (name.startsWith("ANTHROPIC_")) delete process.env[name];
        }
        peer = new BedrockPeer(CREDENTIALS);
        await peer.start();
        runtimeDir = mkdtempSync(join(tmpdir(), "eidnara-bedrock-only-"));
        const harnessRuntime = writeHarnessRuntime({
            dir: runtimeDir,
            sources: sources.sources,
            piInstall: ensurePiInstall(),
            opencodeBaseUrl: peer.http1Url,
            piBaseUrl: peer.h2Url,
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
            mockProviderURL: peer.http1Url,
            existingEnv: env,
            userHostConnectionFile: host.connectionFile,
            modelContextLimit: 30_000,
            bedrock: { baseURL: peer.http1Url, region: CREDENTIALS.region, models: [MODEL] },
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
        await peer?.stop().catch(() => undefined);
        if (runtimeDir) rmSync(runtimeDir, { recursive: true, force: true });
    });

    it("captures a memory through the harness's own Bedrock auth", async () => {
        await prompt(`Please note for this project: ${CAPTURED_FACT}`);
        const status = await waitFor(
            "a completed capture source",
            async () => {
                const status = await host.contextRequest(
                    { project_root: env.workdir, harness: "opencode", session: sessionId },
                    { method: "memory.capture.status", v: 2, session_id: sessionId },
                );
                return typeof status.completed === "number" && status.completed >= 1
                    ? status
                    : undefined;
            },
            120_000,
            diagnostics,
        );
        expect(status.failed).toBe(0);
        expect(served("memory_capture")).toBeGreaterThanOrEqual(1);
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
        expect(served("context_researcher")).toBeGreaterThanOrEqual(1);
    }, 180_000);

    it("classifies the captured memory through the OpenCode ModelExecution closure", async () => {
        const { objectIds, response } = await classifyMemories(
            host,
            { project_root: env.workdir, harness: "opencode", session: sessionId },
            MODEL_REF,
        );
        expect(objectIds.length).toBeGreaterThanOrEqual(1);
        expect(response.classified).toBe(objectIds.length);
        expect(served("memory_classifier")).toBeGreaterThanOrEqual(1);
    }, 300_000);

    it("fires the History Summarizer through the OpenCode ModelExecution closure", async () => {
        peer.conversationInputTokens = 3_000;
        for (let turn = 1; turn <= 10; turn += 1) {
            await prompt(`history turn ${turn}: ${"ballast words ".repeat(400)}`);
        }
        peer.conversationInputTokens = 27_000;
        await prompt(`history trigger: ${"ballast words ".repeat(400)}`);
        peer.conversationInputTokens = 500;
        await prompt("history follow-up");
        await waitFor(
            "a summarizer answer",
            () => (served("history_summarizer") >= 1 ? true : undefined),
            240_000,
            diagnostics,
        );
        const status = await waitFor(
            "a published history segment",
            async () => {
                const status = await host.primaryStatus(sessionId, env.workdir, "session.status");
                return Number(status.history_segment_count ?? 0) > 0 ? status : undefined;
            },
            120_000,
            diagnostics,
        );
        const summarizer = (status.history_summarizer ?? {}) as Record<string, unknown>;
        expect(summarizer.last_failure ?? null).toBeNull();
    }, 600_000);

    it("wraps up the session through the same closure", async () => {
        const before = served("history_summarizer");
        await prompt(`wrapup ballast: ${"ballast words ".repeat(400)}`);
        await command("eidnara-wrapup", "1");
        const result = await waitFor(
            "a wrapup result",
            async () => (await sessionTexts()).find((text) => text.startsWith("## Eidnara Wrapup")),
            300_000,
            diagnostics,
        );
        expect(result).not.toContain("Failed");
        expect(served("history_summarizer")).toBeGreaterThan(before);
    }, 420_000);

    it("reached only the Bedrock peer, signed with the Bedrock row", () => {
        const answered = peer.requests.filter((request) => request.status === 200);
        expect(peer.requests.every((request) => request.signatureValid)).toBe(true);
        expect(
            peer.requests.every((request) => request.sessionToken === CREDENTIALS.sessionToken),
        ).toBe(true);
        expect(
            peer.requests.some((request) =>
                request.headerNames.some((name) => /x-api-key|anthropic/.test(name)),
            ),
        ).toBe(false);
        expect(answered.every((request) => request.transport === "http1")).toBe(true);
        expect(answered.every((request) => request.userAgent.includes("opencode/1.18.22"))).toBe(
            true,
        );
        expect(
            answered
                .filter((request) => request.caller !== "conversation")
                .every((request) => request.modelId === MODEL),
        ).toBe(true);
        expect(new Set(answered.map((request) => request.caller))).toEqual(
            new Set<Caller | undefined>([
                "conversation",
                "memory_capture",
                "context_researcher",
                "memory_classifier",
                "history_summarizer",
            ]),
        );
    });
});

describe.skipIf(sources.ok || !REQUIRED)("bedrock-only callers: OpenCode prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(sources.ok ? "" : sources.reason).toBe("");
    });
});
