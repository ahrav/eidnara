import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
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
import { PiRpcClient, type PiRpcEvent } from "../src/pi-runner/rpc-client";
import { createPiIsolatedEnv, detectPiPrereqs, type PiIsolatedEnv } from "../src/pi-runner/spawn";
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
const piPrereqs = detectPiPrereqs();
const REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";

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

function texts(value: unknown): string[] {
    if (typeof value === "string") return [value];
    if (Array.isArray(value)) return value.flatMap(texts);
    if (value && typeof value === "object") return Object.values(value).flatMap(texts);
    return [];
}

describe.skipIf(!rustPrereqs.ok || !piPrereqs.ok || !sources.ok)("bedrock-only callers: Pi", () => {
    let peer: BedrockPeer;
    let env: PiIsolatedEnv;
    let host: HermeticHostStack;
    let rpc: PiRpcClient;
    let runtimeDir: string;
    let dataDir: string;
    let sessionId: string;

    const diagnostics = (): string =>
        `peer: ${JSON.stringify(peer.requests.map(({ caller, status, transport, systemHead }) => ({ caller, status, transport, systemHead: systemHead.slice(0, 60) })))}\n` +
        `pi stderr:\n${rpc?.getStderr().slice(-3_000)}\nhost log:\n${host?.hostLog().slice(-6_000)}`;

    const served = (caller: Caller): number =>
        peer.callers().filter((served) => served === caller).length;

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

    beforeAll(async () => {
        if (!sources.ok) return;
        for (const name of Object.keys(process.env)) {
            if (name.startsWith("ANTHROPIC_")) delete process.env[name];
        }
        peer = new BedrockPeer(CREDENTIALS);
        await peer.start();
        runtimeDir = mkdtempSync(join(tmpdir(), "eidnara-bedrock-only-pi-"));
        dataDir = mkdtempSync(join(tmpdir(), "eidnara-bedrock-only-pi-data-"));
        const harnessRuntime = writeHarnessRuntime({
            dir: runtimeDir,
            sources: sources.sources,
            piInstall: ensurePiInstall(),
            opencodeBaseUrl: peer.http1Url,
            piBaseUrl: peer.h2Url,
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
            mockProviderURL: peer.h2Url,
            env,
            modelContextLimit: 30_000,
            bedrock: { baseUrl: peer.h2Url, model: MODEL, env: AWS_ENV },
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
        await peer?.stop().catch(() => undefined);
        for (const dir of [runtimeDir, env?.baseDir]) {
            if (dir) rmSync(dir, { recursive: true, force: true });
        }
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
        expect(served("memory_capture")).toBeGreaterThanOrEqual(1);
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
        expect(served("context_researcher")).toBeGreaterThanOrEqual(1);
    }, 180_000);

    it("classifies the captured memory through the Pi ModelExecution closure", async () => {
        const { objectIds, response } = await classifyMemories(host, identity(), MODEL_REF);
        expect(objectIds.length).toBeGreaterThanOrEqual(1);
        expect(response.classified).toBe(objectIds.length);
        expect(served("memory_classifier")).toBeGreaterThanOrEqual(1);
    }, 300_000);

    it("fires the History Summarizer through the Pi ModelExecution closure", async () => {
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
                const status = await host.contextRequest(identity(), {
                    method: "session.status",
                    v: 1,
                    session_id: sessionId,
                });
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
        await rpc.sendCommand("prompt", { message: "/eidnara-wrapup 1" }, { timeoutMs: 300_000 });
        const result = await waitFor(
            "a wrapup result",
            () => {
                const log = existsSync(join(env.baseDir, "eidnara.log"))
                    ? readFileSync(join(env.baseDir, "eidnara.log"), "utf8")
                    : "";
                const at = log.lastIndexOf("/eidnara-wrapup: ## Eidnara Wrapup");
                return at < 0 ? undefined : log.slice(at, log.indexOf("\n", at));
            },
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
        expect(answered.every((request) => request.transport === "h2")).toBe(true);
        expect(answered.every((request) => request.userAgent.includes("aws-sdk-js/"))).toBe(true);
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

describe.skipIf(sources.ok || !REQUIRED)("bedrock-only callers: Pi prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(sources.ok ? "" : sources.reason).toBe("");
    });
});
