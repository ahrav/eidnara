import { expect } from "bun:test";
import type { Caller } from "./answers";
import { BedrockPeer, type CapturedBedrockRequest } from "./server";

export const MODEL = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
export const MODEL_REF = `amazon-bedrock/${MODEL}`;
export const CREDENTIALS = {
    accessKeyId: "AKIDBEDROCKONLYE2E",
    secretAccessKey: "bedrock/only+e2e/secret",
    sessionToken: "bedrock-only-e2e-session-token",
    region: "us-east-1",
};
export const AWS_ENV: Record<string, string> = {
    AWS_ACCESS_KEY_ID: CREDENTIALS.accessKeyId,
    AWS_SECRET_ACCESS_KEY: CREDENTIALS.secretAccessKey,
    AWS_SESSION_TOKEN: CREDENTIALS.sessionToken,
    AWS_REGION: CREDENTIALS.region,
};
export const REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";

/** `live` answers the harness session, `closure` answers ModelExecution runs, and `sentinel` stands where an Anthropic request would go. */
export interface Peers {
    live: BedrockPeer;
    closure: BedrockPeer;
    sentinel: BedrockPeer;
}

export async function startPeers(): Promise<Peers> {
    const peers = {
        live: new BedrockPeer(CREDENTIALS),
        closure: new BedrockPeer(CREDENTIALS),
        sentinel: new BedrockPeer(CREDENTIALS),
    };
    await Promise.all(Object.values(peers).map((peer) => peer.start()));
    return peers;
}

export async function stopPeers(peers: Peers | undefined): Promise<void> {
    if (!peers) return;
    await Promise.all(Object.values(peers).map((peer) => peer.stop().catch(() => undefined)));
}

/** Removes every `ANTHROPIC_*` and `AWS_*` variable from the runner's environment and returns the restore. */
export function isolateProviderEnv(): () => void {
    const saved: Record<string, string> = {};
    for (const [name, value] of Object.entries(process.env)) {
        if (value === undefined || !/^(ANTHROPIC|AWS)_/.test(name)) continue;
        saved[name] = value;
        delete process.env[name];
    }
    return () => Object.assign(process.env, saved);
}

export const served = (peer: BedrockPeer, caller: Caller): number =>
    peer.callers().filter((served) => served === caller).length;

export function peerDiagnostics(peers: Peers | undefined): string {
    if (!peers) return "";
    return Object.entries(peers)
        .map(
            ([name, peer]) =>
                `${name}: ${JSON.stringify(peer.requests.map(({ caller, status, transport }: CapturedBedrockRequest) => ({ caller, status, transport })))}`,
        )
        .join("\n");
}

export async function waitFor<T>(
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

/** The rounds a completed `/eidnara-wrapup` result reports; a Partial, Skipped, Failed, starting, or nothing-to-compact result reports none. */
export function completedWrapupRounds(text: string): number | undefined {
    const match =
        /^## Eidnara Wrapup\s+(?!—|Starting wrapup|Nothing to compact)[^#]*?\((\d+) rounds?\)/.exec(
            text,
        );
    return match ? Number(match[1]) : undefined;
}

/** A Starting notice is a progress line; every other `/eidnara-wrapup` result settles one run. */
export const isWrapupResult = (text: string): boolean =>
    text.startsWith("## Eidnara Wrapup") && !/^## Eidnara Wrapup\s+Starting wrapup/.test(text);

/** Every request was signed with the Bedrock row and reached the peer its path names; nothing reached the sentinel. */
export function auditPeers(
    peers: Peers,
    harness: { transport: "http1" | "h2"; userAgent: string },
): void {
    expect(peers.sentinel.requests).toEqual([]);
    const requests = [...peers.live.requests, ...peers.closure.requests];
    const summary = JSON.stringify(
        requests.map(({ caller, status, signatureValid, transport, userAgent, modelId }) => ({
            caller,
            status,
            signatureValid,
            transport,
            userAgent: userAgent.slice(0, 48),
            modelId,
        })),
    );
    expect(
        requests.every((request) => request.signatureValid && request.status === 200),
        summary,
    ).toBe(true);
    expect(
        requests.some((request) =>
            request.headerNames.some((name) => /x-api-key|anthropic/.test(name)),
        ),
    ).toBe(false);
    expect(
        requests.every((request) => request.transport === harness.transport),
        summary,
    ).toBe(true);
    expect(
        requests.every((request) => request.userAgent.includes(harness.userAgent)),
        summary,
    ).toBe(true);
    expect(
        requests
            .filter((request) => request.caller !== "conversation")
            .every((request) => request.modelId === MODEL),
    ).toBe(true);
    const live = new Set(peers.live.callers());
    const closure = new Set(peers.closure.callers());
    for (const caller of ["conversation", "memory_capture", "context_researcher"] as const) {
        expect(live.has(caller)).toBe(true);
    }
    for (const caller of ["history_summarizer", "memory_classifier"] as const) {
        expect(closure.has(caller)).toBe(true);
        expect(live.has(caller)).toBe(false);
    }
    expect(
        [...closure].every(
            (caller) => caller !== "memory_capture" && caller !== "context_researcher",
        ),
    ).toBe(true);
}
