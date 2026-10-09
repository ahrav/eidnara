import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "../compression-fidelity/corpus";
import { __spawnOpencodeTest, createIsolatedEnv } from "../opencode-runner/spawn";
import type { CassetteOracle } from "./cassette-oracle";
import { type ForwardConfig, publishForwardingReport, validateForwardConfig } from "./forward";
import { MockProvider } from "./server";

const UPSTREAM = "https://provider.test/v1/messages";
const MODEL = "claude-test-model";
const FAKE_KEY = "sk-ant-api03-NEGATIVE-CONTROL-not-a-real-key";
const USAGE = { input_tokens: 40, output_tokens: 6 };

function outboundSpy(): { calls: string[]; restore: () => void } {
    const original = globalThis.fetch;
    const calls: string[] = [];
    const spy = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = new URL(input instanceof Request ? input.url : String(input));
        if (url.hostname !== "127.0.0.1") calls.push(url.href);
        return original(input, init);
    }) as typeof fetch;
    globalThis.fetch = spy;
    return { calls, restore: () => (globalThis.fetch = original) };
}

function sse(content: unknown[], stopReason: string, usage = USAGE): string {
    const frames = [
        {
            type: "message_start",
            message: { id: "msg_1", model: MODEL, usage: { ...usage, output_tokens: 0 } },
        },
        ...content.flatMap((block, index) => [
            { type: "content_block_start", index, content_block: block },
            { type: "content_block_stop", index },
        ]),
        { type: "message_delta", delta: { stop_reason: stopReason }, usage },
        { type: "message_stop" },
    ];
    return frames.map((f) => `event: ${f.type}\ndata: ${JSON.stringify(f)}\n\n`).join("");
}

interface Received {
    url: string;
    headers: Record<string, string>;
    body: Uint8Array;
    redirect: RequestInit["redirect"];
}

function upstreamDouble(replies: Array<() => Promise<Response> | Response>) {
    const received: Received[] = [];
    const send = async (url: string, init: RequestInit): Promise<Response> => {
        const headers: Record<string, string> = {};
        new Headers(init.headers).forEach((value, key) => {
            headers[key] = value;
        });
        received.push({
            url,
            headers,
            body: new Uint8Array(init.body as Uint8Array),
            redirect: init.redirect,
        });
        const reply = replies.shift();
        if (!reply) throw new Error("the double has no reply left");
        return reply();
    };
    return { received, send };
}

function config(overrides: Partial<ForwardConfig> = {}): ForwardConfig {
    return {
        upstreamURL: UPSTREAM,
        model: MODEL,
        corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
        pricing: { inputPerMTok: 3, outputPerMTok: 15 },
        limits: { maxCalls: 4, maxOutputTokens: 1024, timeoutMs: 2_000, spendCapUsd: 1 },
        credentials: () => ({ "x-api-key": FAKE_KEY }),
        ...overrides,
    };
}

const started: MockProvider[] = [];
async function start(mock: MockProvider): Promise<string> {
    started.push(mock);
    return (await mock.start()).baseURL;
}

afterEach(async () => {
    for (const mock of started.splice(0)) await mock.stop();
});

function post(baseURL: string, body: unknown): Promise<Response> {
    return fetch(`${baseURL}/v1/messages`, {
        method: "POST",
        headers: {
            "content-type": "application/json",
            "anthropic-version": "2023-06-01",
            "x-api-key": "test-key-not-real",
        },
        body: typeof body === "string" ? body : JSON.stringify(body),
    });
}

const firstTurn = {
    model: MODEL,
    max_tokens: 512,
    stream: true,
    messages: [{ role: "user", content: "read the file" }],
};
const toolResultTurn = {
    ...firstTurn,
    messages: [
        ...firstTurn.messages,
        {
            role: "assistant",
            content: [{ type: "tool_use", id: "toolu_1", name: "read", input: {} }],
        },
        {
            role: "user",
            content: [{ type: "tool_result", tool_use_id: "toolu_1", content: "body" }],
        },
    ],
};

describe("scripted mode", () => {
    test("sends nothing off the host when scripts exhaust, scripts are invalid, or setup fails", async () => {
        const spy = outboundSpy();
        try {
            const exhausted = new MockProvider();
            const base = await start(exhausted);
            expect((await post(base, firstTurn)).status).toBe(500);
            exhausted.script([{ text: "no usage" }]);
            expect((await post(base, firstTurn)).status).toBe(500);
            expect((await post(base, "{not json")).status).toBe(500);

            const refusing: Pick<CassetteOracle, "lookup" | "record"> = {
                lookup: async () => {
                    throw new Error("oracle down");
                },
                record: async () => {
                    throw new Error("oracle down");
                },
            };
            const failedSetup = new MockProvider();
            failedSetup.useCassette({ oracle: refusing, mode: "replay", namespace: "unit" });
            expect((await post(await start(failedSetup), firstTurn)).status).toBe(400);
        } finally {
            spy.restore();
        }
        expect(spy.calls).toEqual([]);
    });
});

describe("construction", () => {
    test("forwarding with scripted responses is refused", () => {
        expect(() => new MockProvider({ forward: config(), script: [{ usage: USAGE }] })).toThrow(
            "mutually exclusive",
        );
        const forwarding = new MockProvider({ forward: config() });
        expect(() => forwarding.script([{ usage: USAGE }])).toThrow("no scripted responses");
        expect(() => forwarding.enqueue({ usage: USAGE })).toThrow("no scripted responses");
        expect(() => forwarding.setDefault({ usage: USAGE })).toThrow("no scripted responses");
        expect(() => forwarding.addMatcher(() => null)).toThrow("no scripted responses");
        expect(() => new MockProvider().forwardingReport()).toThrow("not forwarding");
    });

    test("forwarding needs an HTTPS Messages provider, a model, prices, and all four limits", () => {
        expect(() => validateForwardConfig(config())).not.toThrow();
        const refused: Array<[Partial<ForwardConfig>, string]> = [
            [{ upstreamURL: "http://provider.test/v1/messages" }, "HTTPS"],
            [{ upstreamURL: "" }, "explicit upstream URL"],
            [{ upstreamURL: "https://provider.test/v1/chat/completions" }, "Messages endpoint"],
            [{ model: "" }, "explicit model"],
            [{ corpusSha256: "0".repeat(64) }, "reviewed synthetic corpus"],
            [{ pricing: { inputPerMTok: 0, outputPerMTok: 15 } }, "prices"],
            [{ credentials: undefined as unknown as ForwardConfig["credentials"] }, "credential"],
        ];
        for (const name of ["maxCalls", "maxOutputTokens", "timeoutMs", "spendCapUsd"] as const) {
            const limits = { ...config().limits } as Record<string, number | undefined>;
            delete limits[name];
            refused.push([{ limits: limits as unknown as ForwardConfig["limits"] }, name]);
        }
        refused.push([{ limits: { ...config().limits, maxCalls: 1.5 } }, "whole numbers"]);
        for (const [overrides, message] of refused) {
            expect(() => new MockProvider({ forward: config(overrides) })).toThrow(message);
        }
    });
});

describe("forwarding", () => {
    test("forwards the received bytes, returns the provider's response, and captures the tool-result turn", async () => {
        const toolUse = sse(
            [{ type: "tool_use", id: "toolu_1", name: "read", input: {} }],
            "tool_use",
        );
        const final = sse([{ type: "text", text: "done" }], "end_turn");
        const double = upstreamDouble([
            () => new Response(toolUse, { headers: { "content-type": "text/event-stream" } }),
            () => new Response(final, { headers: { "content-type": "text/event-stream" } }),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        const base = await start(mock);
        const sent = [JSON.stringify(firstTurn), JSON.stringify(toolResultTurn)];
        const first = await post(base, sent[0]);
        expect(await first.text()).toBe(toolUse);
        const second = await post(base, sent[1]);
        expect(await second.text()).toBe(final);

        expect(double.received.map((r) => new TextDecoder().decode(r.body))).toEqual(sent);
        expect(double.received.every((r) => r.url === UPSTREAM && r.redirect === "error")).toBe(
            true,
        );
        expect(double.received[0]?.headers["x-api-key"]).toBe(FAKE_KEY);
        expect(double.received[0]?.headers["anthropic-version"]).toBe("2023-06-01");
        const report = mock.forwardingReport();
        expect(report.exchanges.map((e) => e.request.body_text)).toEqual(sent);
        expect(report.exchanges[1]?.request.body_sha256).toBe(
            new Bun.CryptoHasher("sha256").update(sent[1] ?? "").digest("hex"),
        );
        expect(report.exchanges.map((e) => e.response?.stop_reason)).toEqual([
            "tool_use",
            "end_turn",
        ]);
        expect(mock.requests()[1]?.body.messages?.at(-1)?.content).toEqual(
            toolResultTurn.messages.at(-1)?.content,
        );
        expect(report.attempted_sends).toBe(2);
        expect(report.acknowledged_responses).toBe(2);
        expect(report.complete).toBe(true);
        expect(report.mode).toBe("forward");
        expect(report.spent_usd).toBeCloseTo((2 * (40 * 3 + 6 * 15)) / 1_000_000, 12);
    });

    test("each limit stops the run instead of extending it", async () => {
        const ok = () =>
            new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                headers: { "content-type": "text/event-stream" },
            });
        const runs: Array<{
            forward: Partial<ForwardConfig>;
            body: unknown;
            replies: Array<() => Promise<Response> | Response>;
            sends: number;
            stopped: string;
        }> = [
            {
                forward: { limits: { ...config().limits, maxCalls: 1 } },
                body: firstTurn,
                replies: [ok],
                sends: 1,
                stopped: "maxCalls",
            },
            {
                forward: {},
                body: { ...firstTurn, max_tokens: 4096 },
                replies: [],
                sends: 0,
                stopped: "maxOutputTokens",
            },
            {
                forward: {},
                body: { ...firstTurn, model: "another" },
                replies: [],
                sends: 0,
                stopped: "another",
            },
            {
                forward: { limits: { ...config().limits, spendCapUsd: 0.000001 } },
                body: firstTurn,
                replies: [],
                sends: 0,
                stopped: "spendCapUsd",
            },
            {
                forward: { limits: { ...config().limits, timeoutMs: 50 } },
                body: firstTurn,
                replies: [() => new Promise<Response>(() => {})],
                sends: 1,
                stopped: "without a response",
            },
            {
                forward: {},
                body: firstTurn,
                replies: [() => new Response('{"type":"error"}', { status: 529 })],
                sends: 1,
                stopped: "HTTP 529",
            },
        ];
        for (const run of runs) {
            const double = upstreamDouble(run.replies);
            const mock = new MockProvider({
                forward: config({ ...run.forward, fetch: double.send }),
            });
            const base = await start(mock);
            for (let turn = 0; turn < 3; turn += 1) await (await post(base, run.body)).text();
            const report = mock.forwardingReport();
            expect(double.received.length).toBe(run.sends);
            expect(report.attempted_sends).toBe(run.sends);
            expect(report.stopped).toContain(run.stopped);
            expect(report.complete).toBe(false);
        }
    });

    test("an ambiguous send or missing usage is charged its reservation, never zero", async () => {
        const noUsage = () =>
            new Response(
                JSON.stringify({ type: "message", stop_reason: "end_turn", content: [] }),
                {
                    headers: { "content-type": "application/json" },
                },
            );
        const double = upstreamDouble([noUsage]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        await (await post(await start(mock), { ...firstTurn, stream: false })).text();
        const report = mock.forwardingReport();
        const exchange = report.exchanges[0];
        expect(exchange?.response?.cost_known).toBe(false);
        expect(exchange?.response?.cost_usd).toBe(exchange?.request.reserved_usd ?? -1);
        expect(report.spent_usd).toBeGreaterThan(0);
        expect(report.incomplete_reasons).toContain("cost unknown for a send");
    });

    test("an unfinished tool loop is incomplete", async () => {
        const toolUse = sse(
            [{ type: "tool_use", id: "toolu_1", name: "read", input: {} }],
            "tool_use",
        );
        const double = upstreamDouble([
            () => new Response(toolUse, { headers: { "content-type": "text/event-stream" } }),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        await (await post(await start(mock), firstTurn)).text();
        expect(mock.forwardingReport().incomplete_reasons).toContain("tool loop unfinished");
    });

    test("captures, the report file, and generated configuration hold no credential", async () => {
        const double = upstreamDouble([
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream", "set-cookie": FAKE_KEY },
                }),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        await (await post(await start(mock), firstTurn)).text();
        expect(double.received[0]?.headers["x-api-key"]).toBe(FAKE_KEY);

        const dir = mkdtempSync(join(tmpdir(), "forwarding-"));
        try {
            const target = join(dir, "private");
            const path = publishForwardingReport(mock.forwardingReport(), target, "probe");
            expect(statSync(target).mode & 0o777).toBe(0o700);
            expect(statSync(path).mode & 0o777).toBe(0o600);
            const published = readFileSync(path, "utf8");
            expect(published).not.toContain(FAKE_KEY);
            expect(published).not.toContain("test-key-not-real");
            expect(published).toContain("[redacted]");
            expect(JSON.stringify(mock.requests())).not.toContain(FAKE_KEY);
            expect(JSON.stringify(mock.requests())).not.toContain("test-key-not-real");
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
        expect(() =>
            publishForwardingReport(mock.forwardingReport(), resolve(import.meta.dir), "inside"),
        ).toThrow("inside the repository");

        const env = createIsolatedEnv();
        try {
            __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                mockProviderURL: "http://127.0.0.1:4321",
                modelId: MODEL,
                modelContextLimit: 200_000,
            });
            const generated = readFileSync(join(env.configDir, "opencode.json"), "utf8");
            expect(generated).toContain(`"${MODEL}"`);
            expect(generated).not.toContain(FAKE_KEY);
        } finally {
            rmSync(resolve(env.configDir, ".."), { recursive: true, force: true });
        }
    });
});
