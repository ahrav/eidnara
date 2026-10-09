import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "../compression-fidelity/corpus";
import { __spawnOpencodeTest, createIsolatedEnv } from "../opencode-runner/spawn";
import type { CassetteOracle } from "./cassette-oracle";
import {
    type ForwardConfig,
    Forwarder,
    MAX_RESPONSE_BYTES,
    publishForwardingReport,
    readResponse,
    validateForwardConfig,
} from "./forward";
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
        if (url.hostname === "127.0.0.1") return original(input, init);
        calls.push(url.href);
        throw new TypeError(`outbound fetch to ${url.host} is blocked in tests`);
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
        contextLimit: 100_000,
        corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
        pricing: { inputPerMTok: 3, outputPerMTok: 15 },
        limits: { maxCalls: 4, maxOutputTokens: 1024, timeoutMs: 2_000, spendCapUsd: 1 },
        credentials: () => ({ "x-api-key": FAKE_KEY }),
        ...overrides,
    };
}

const started: MockProvider[] = [];
const keys = new Map<string, string>();
async function start(mock: MockProvider): Promise<string> {
    started.push(mock);
    const base = (await mock.start()).baseURL;
    keys.set(base, mock.inboundKey);
    return base;
}

afterEach(async () => {
    for (const mock of started.splice(0)) await mock.stop();
});

function post(baseURL: string, body: unknown, key = keys.get(baseURL)): Promise<Response> {
    return fetch(`${baseURL}/v1/messages`, {
        method: "POST",
        headers: {
            "content-type": "application/json",
            "anthropic-version": "2023-06-01",
            "x-api-key": key ?? "",
        },
        body:
            typeof body === "string" || body instanceof Uint8Array
                ? (body as BodyInit)
                : JSON.stringify(body),
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

    test("the outbound spy sees a forwarding mock's send", async () => {
        const spy = outboundSpy();
        try {
            const mock = new MockProvider({
                forward: config({ upstreamURL: "https://provider.invalid/v1/messages" }),
            });
            await (await post(await start(mock), firstTurn)).text();
            expect(mock.forwardingReport().stopped).toContain("without a response");
        } finally {
            spy.restore();
        }
        expect(spy.calls).toEqual(["https://provider.invalid/v1/messages"]);
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
            [{ contextLimit: 0 }, "context limit"],
            [{ upstreamURL: "https://user:pass@provider.test/v1/messages" }, "no credential"],
            [{ upstreamURL: "https://provider.test/v1/messages?key=x" }, "no credential"],
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
    test("a response is read from its events however their keys are spelled", () => {
        const stream = [
            'data: {"type":"message_start","message":{"usage":{"input_tokens":40,"output_tokens":0}}}',
            'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"the \\"usage\\" word"}}',
            'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"plain"}}',
            'data: {"type":"content_block_start","content_bloc\\u006b":{"type":"tool_use","id":"toolu_esc"}}',
            'data: {"type":"message_delta","delta":{"stop_reaso\\u006e":"tool_use"},"\\u0075sage":{"output_tokens":6}}',
            "data: 7",
            "data: not json",
            'data: {"type":"message_sto\\u0070"}',
        ].join("\n");
        expect(readResponse("text/event-stream", stream)).toEqual({
            usage: {
                input_tokens: 40,
                output_tokens: 6,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            stopReason: "tool_use",
            model: null,
            toolUses: ["toolu_esc"],
            unfinished: null,
            malformed: null,
        });
        expect(
            readResponse("text/event-stream", stream.split("\n").slice(1, 3).join("\n")),
        ).toEqual({
            usage: null,
            stopReason: null,
            model: null,
            toolUses: [],
            unfinished: "stream ended before message_stop",
            malformed: "is not a Messages message",
        });
    });

    test("a stream cut short or carrying an error event states no usage", () => {
        const opening =
            'data: {"type":"message_start","message":{"usage":{"input_tokens":40,"output_tokens":1}}}';
        const delta =
            'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}}';
        const cut = [opening, delta].join("\n");
        expect(readResponse("text/event-stream", cut)).toEqual({
            usage: null,
            stopReason: null,
            model: null,
            toolUses: [],
            unfinished: "stream ended before message_stop",
            malformed: null,
        });
        const errored = [
            opening,
            delta,
            'data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}',
            'data: {"type":"message_stop"}',
        ].join("\n");
        expect(readResponse("text/event-stream", errored).usage).toBeNull();
        expect(readResponse("text/event-stream", errored).unfinished).toBe(
            "stream carried an error event",
        );
        const provisional = [opening, delta, 'data: {"type":"message_stop"}'].join("\n");
        expect(readResponse("text/event-stream", provisional)).toEqual({
            usage: null,
            stopReason: null,
            model: null,
            toolUses: [],
            unfinished: null,
            malformed: null,
        });
    });

    test("an interrupted or errored stream keeps its reservation and stops the run", async () => {
        const opening = {
            type: "message_start",
            message: { id: "msg_1", model: MODEL, usage: { ...USAGE, output_tokens: 1 } },
        };
        const frames = (events: unknown[]) =>
            events.map((e) => `data: ${JSON.stringify(e)}\n\n`).join("");
        const delta = {
            type: "content_block_delta",
            index: 0,
            delta: { type: "text_delta", text: "partial" },
        };
        const streams: Array<[string, string]> = [
            [frames([opening, delta]), "before message_stop"],
            [
                frames([
                    opening,
                    delta,
                    { type: "error", error: { type: "overloaded_error", message: "Overloaded" } },
                ]),
                "an error event",
            ],
        ];
        for (const [stream, reason] of streams) {
            const double = upstreamDouble([
                () => new Response(stream, { headers: { "content-type": "text/event-stream" } }),
                () =>
                    new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                        headers: { "content-type": "text/event-stream" },
                    }),
            ]);
            const mock = new MockProvider({ forward: config({ fetch: double.send }) });
            const base = await start(mock);
            expect(await (await post(base, firstTurn)).text()).toBe(stream);
            expect((await post(base, firstTurn)).status).toBe(400);
            const report = mock.forwardingReport();
            const exchange = report.exchanges[0];
            expect(double.received.length).toBe(1);
            expect(exchange?.response?.cost_known).toBe(false);
            expect(exchange?.response?.cost_usd).toBe(exchange?.request.reserved_usd ?? -1);
            expect(report.spent_usd).toBe(exchange?.request.reserved_usd ?? -1);
            expect(report.stopped).toContain(reason);
            expect(report.complete).toBe(false);
        }
    });

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
        // Whitespace, key order, an escape, and a raw non-ASCII letter that re-serializing
        // the parsed body would change.
        const firstBytes = new TextEncoder().encode(
            `{ "stream" : true,"messages":[{"role":"user","content":"caf\\u00e9 é read the file"}],\n "max_tokens":512, "model":"${MODEL}" }`,
        );
        const secondBytes = new TextEncoder().encode(JSON.stringify(toolResultTurn));
        const first = await post(base, firstBytes);
        expect(await first.text()).toBe(toolUse);
        const second = await post(base, secondBytes);
        expect(await second.text()).toBe(final);

        expect(double.received.map((r) => r.body)).toEqual([firstBytes, secondBytes]);
        expect(double.received.every((r) => r.url === UPSTREAM && r.redirect === "error")).toBe(
            true,
        );
        expect(double.received[0]?.headers).toEqual({
            accept: "*/*",
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
            "x-api-key": FAKE_KEY,
        });
        const report = mock.forwardingReport();
        expect(report.exchanges.map((e) => e.request.body_sha256)).toEqual(
            [firstBytes, secondBytes].map((bytes) =>
                new Bun.CryptoHasher("sha256").update(bytes).digest("hex"),
            ),
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
        expect(report.incomplete_reasons).toEqual([]);
        expect(report.complete).toBe(true);
        expect(report.mode).toBe("forward");
        expect(report.spent_usd).toBeCloseTo((2 * (40 * 3 + 6 * 15)) / 1_000_000, 12);
    });

    test("a report names each body's SHA-256 before and after its digest settles", async () => {
        const double = upstreamDouble([
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream" },
                }),
            () =>
                new Response(sse([{ type: "text", text: "y" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream" },
                }),
        ]);
        const forwarder = new Forwarder(config({ fetch: double.send }));
        const bodies = [firstTurn, { ...firstTurn, max_tokens: 256 }].map((turn) =>
            new TextEncoder().encode(JSON.stringify(turn)),
        );
        const send = (bytes: Uint8Array<ArrayBuffer>) => {
            const text = new TextDecoder().decode(bytes);
            return forwarder.forward(bytes, text, JSON.parse(text), {});
        };
        const hashes = bodies.map((bytes) =>
            new Bun.CryptoHasher("sha256").update(bytes).digest("hex"),
        );

        const pending = send(bodies[0] as Uint8Array<ArrayBuffer>);
        expect(forwarder.report().exchanges[0]?.request.body_sha256).toBe(hashes[0]);
        await (await pending).text();
        await (await send(bodies[1] as Uint8Array<ArrayBuffer>)).text();
        await Bun.sleep(20);
        const report = forwarder.report();
        expect(report.exchanges.map((e) => e.request.body_sha256)).toEqual(hashes);
        expect(report.complete).toBe(true);
    });

    test("a request without this mock's key is refused before any send", async () => {
        const double = upstreamDouble([]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        const base = await start(mock);
        expect((await post(base, firstTurn, "test-key-not-real")).status).toBe(401);
        expect((await post(base, firstTurn, "")).status).toBe(401);
        expect(double.received).toEqual([]);
        expect(mock.forwardingReport().attempted_sends).toBe(0);
        expect(mock.forwardingReport().stopped).toBeNull();
    });

    test("every mock draws its own inbound key", () => {
        const keys = new Set(
            Array.from({ length: 4 }, () => new MockProvider({ forward: config() }).inboundKey),
        );
        expect(keys.size).toBe(4);
    });

    test("a failing credential callback refuses the send and stops the run", async () => {
        const double = upstreamDouble([]);
        const mock = new MockProvider({
            forward: config({
                fetch: double.send,
                credentials: () => {
                    throw new Error("no credential");
                },
            }),
        });
        const base = await start(mock);
        expect((await post(base, firstTurn)).status).toBe(400);
        expect((await post(base, firstTurn)).status).toBe(400);
        const report = mock.forwardingReport();
        expect(double.received).toEqual([]);
        expect(report.attempted_sends).toBe(0);
        expect(report.spent_usd).toBe(0);
        expect(report.stopped).toContain("credential callback");
    });

    test("the spend cap totals every send and settles each to its stated usage", async () => {
        const reply =
            (usage = USAGE) =>
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn", usage), {
                    headers: { "content-type": "text/event-stream" },
                });
        const bytes = Buffer.byteLength(JSON.stringify(firstTurn));
        const reservation = (bytes * 3 + 512 * 15) / 1_000_000;
        const settled = (40 * 3 + 6 * 15) / 1_000_000;

        const unsettled = upstreamDouble([
            reply({ input_tokens: bytes, output_tokens: 512 }),
            reply(),
        ]);
        const capped = new MockProvider({
            forward: config({
                fetch: unsettled.send,
                limits: { ...config().limits, spendCapUsd: reservation * 1.5 },
            }),
        });
        const cappedBase = await start(capped);
        for (let turn = 0; turn < 2; turn += 1) await (await post(cappedBase, firstTurn)).text();
        expect(unsettled.received.length).toBe(1);
        expect(capped.forwardingReport().stopped).toContain("spendCapUsd");

        const refunded = upstreamDouble([reply(), reply()]);
        const fits = new MockProvider({
            forward: config({
                fetch: refunded.send,
                limits: { ...config().limits, spendCapUsd: (reservation + settled) * 1.000001 },
            }),
        });
        const fitsBase = await start(fits);
        for (let turn = 0; turn < 2; turn += 1) await (await post(fitsBase, firstTurn)).text();
        expect(refunded.received.length).toBe(2);
        expect(fits.forwardingReport().complete).toBe(true);
    });

    test("a response naming another model is recorded and stops the run", async () => {
        const other = sse([{ type: "text", text: "x" }], "end_turn").replaceAll(
            `"model":"${MODEL}"`,
            '"model":"routed-elsewhere"',
        );
        const double = upstreamDouble([
            () => new Response(other, { headers: { "content-type": "text/event-stream" } }),
            () =>
                new Response(
                    JSON.stringify({
                        type: "message",
                        model: "routed-elsewhere",
                        stop_reason: "end_turn",
                        content: [],
                        usage: USAGE,
                    }),
                    { headers: { "content-type": "application/json" } },
                ),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        const base = await start(mock);
        expect((await post(base, firstTurn)).status).toBe(200);
        expect((await post(base, { ...firstTurn, stream: false })).status).toBe(400);
        const report = mock.forwardingReport();
        expect(report.exchanges[0]?.response?.model).toBe("routed-elsewhere");
        expect(report.stopped).toBe("send 0 names model routed-elsewhere");
        expect(report.complete).toBe(false);
        expect(double.received.length).toBe(1);

        const unnamed = upstreamDouble([
            () =>
                new Response(
                    JSON.stringify({
                        type: "message",
                        stop_reason: "end_turn",
                        content: [],
                        usage: USAGE,
                    }),
                    { headers: { "content-type": "application/json" } },
                ),
        ]);
        const silent = new MockProvider({ forward: config({ fetch: unnamed.send }) });
        await (await post(await start(silent), { ...firstTurn, stream: false })).text();
        expect(silent.forwardingReport().exchanges[0]?.response?.model).toBeNull();
        expect(silent.forwardingReport().stopped).toBe("send 0 states no model");
    });

    test("a request at exactly limits.maxOutputTokens is sent", async () => {
        const double = upstreamDouble([
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream" },
                }),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        await (await post(await start(mock), { ...firstTurn, max_tokens: 1024 })).text();
        expect(double.received.length).toBe(1);
    });

    test("usage above the reservation stops the run", async () => {
        const double = upstreamDouble([
            () =>
                new Response(
                    sse([{ type: "text", text: "x" }], "end_turn", {
                        input_tokens: 10_000_000,
                        output_tokens: 6,
                    }),
                    { headers: { "content-type": "text/event-stream" } },
                ),
        ]);
        const mock = new MockProvider({
            forward: config({ fetch: double.send, limits: { ...config().limits, spendCapUsd: 1 } }),
        });
        const base = await start(mock);
        await (await post(base, firstTurn)).text();
        expect((await post(base, firstTurn)).status).toBe(400);
        const report = mock.forwardingReport();
        expect(report.stopped).toContain("above its reservation");
        expect(report.incomplete_reasons).toContain("spend above the cap");
        expect(report.complete).toBe(false);
        expect(double.received.length).toBe(1);
    });

    test("a report is a snapshot that later sends and edits to an earlier report leave unchanged", async () => {
        const double = upstreamDouble([
            () =>
                new Response(
                    sse([{ type: "tool_use", id: "toolu_1", name: "read", input: {} }], "tool_use"),
                    { headers: { "content-type": "text/event-stream", "x-trace": "t1" } },
                ),
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream" },
                }),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        const base = await start(mock);
        await (await post(base, firstTurn)).text();
        const first = mock.forwardingReport();
        const pristine = structuredClone(first);
        await (await post(base, toolResultTurn)).text();
        expect(first).toEqual(pristine);

        const edited = mock.forwardingReport();
        const exchange = edited.exchanges[0];
        const response = exchange?.response;
        if (!exchange || !response?.usage) throw new Error("the first exchange has no usage");
        exchange.tool_uses.push("toolu_edit");
        exchange.tool_results.push("toolu_edit");
        exchange.request.headers["x-edit"] = "1";
        exchange.request.body_text = "edited";
        response.headers["x-edit"] = "1";
        response.usage.output_tokens = 999;
        response.stop_reason = "edited";
        edited.refusals.push("edited");
        edited.limits.maxCalls = 999;

        const after = mock.forwardingReport();
        expect(after.exchanges[0]).toEqual(pristine.exchanges[0]);
        expect(after.exchanges[0]?.response?.headers["x-trace"]).toBe("t1");
        expect(after.refusals).toEqual([]);
        expect(after.limits.maxCalls).toBe(config().limits.maxCalls);
        expect(after.exchanges.length).toBe(2);
        expect(after.complete).toBe(true);
    });

    test("a send in flight, a missing stop reason, or an unanswered tool call leaves the run incomplete", async () => {
        let release: (response: Response) => void = () => {};
        const pending = upstreamDouble([
            () =>
                new Promise<Response>((resolve) => {
                    release = resolve;
                }),
        ]);
        const inFlight = new MockProvider({ forward: config({ fetch: pending.send }) });
        const reply = post(await start(inFlight), firstTurn);
        while (pending.received.length === 0) await Bun.sleep(5);
        expect(inFlight.forwardingReport().incomplete_reasons).toContain("a send is in flight");
        release(
            new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                headers: { "content-type": "text/event-stream" },
            }),
        );
        await (await reply).text();
        expect(inFlight.forwardingReport().complete).toBe(true);

        const interleaved = upstreamDouble([
            () =>
                new Response(
                    sse([{ type: "tool_use", id: "toolu_9", name: "read", input: {} }], "tool_use"),
                    { headers: { "content-type": "text/event-stream" } },
                ),
            () =>
                new Response(
                    JSON.stringify({ type: "message", model: MODEL, content: [], usage: USAGE }),
                    {
                        headers: { "content-type": "application/json" },
                    },
                ),
        ]);
        const answered = upstreamDouble([
            () =>
                new Response(
                    JSON.stringify({
                        type: "message",
                        model: MODEL,
                        content: [{ input: {}, name: "read", id: "toolu_7", type: "tool_use" }],
                        stop_reason: "tool_use",
                        usage: USAGE,
                    }),
                    { headers: { "content-type": "application/json" } },
                ),
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream" },
                }),
        ]);
        const loop = new MockProvider({ forward: config({ fetch: answered.send }) });
        const loopBase = await start(loop);
        await (await post(loopBase, { ...firstTurn, stream: false })).text();
        const spaced = JSON.stringify(
            {
                ...firstTurn,
                messages: [
                    {
                        role: "user",
                        content: [{ content: "x", tool_use_id: "toolu_7", type: "tool_result" }],
                    },
                ],
            },
            null,
            2,
        );
        await (await post(loopBase, spaced)).text();
        expect(loop.forwardingReport().complete).toBe(true);

        const reordered = upstreamDouble([
            () =>
                new Response(
                    JSON.stringify({
                        type: "message",
                        model: MODEL,
                        content: [{ input: {}, name: "read", id: "toolu_8", type: "tool_use" }],
                        stop_reason: "tool_use",
                        usage: USAGE,
                    }),
                    { headers: { "content-type": "application/json" } },
                ),
            () =>
                new Response(sse([{ type: "text", text: "x" }], "end_turn"), {
                    headers: { "content-type": "text/event-stream" },
                }),
        ]);
        const open = new MockProvider({ forward: config({ fetch: reordered.send }) });
        const openBase = await start(open);
        await (await post(openBase, { ...firstTurn, stream: false })).text();
        await (await post(openBase, firstTurn)).text();
        expect(open.forwardingReport().incomplete_reasons).toContain("tool loop unfinished");

        const side = new MockProvider({ forward: config({ fetch: interleaved.send }) });
        const sideBase = await start(side);
        await (await post(sideBase, firstTurn)).text();
        await (await post(sideBase, { ...firstTurn, stream: false })).text();
        const report = side.forwardingReport();
        expect(report.incomplete_reasons).toContain("tool loop unfinished");
        expect(report.incomplete_reasons).toContain("a response states no stop reason");
        expect(report.complete).toBe(false);
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
                forward: {},
                body: `{"model":"${MODEL}","max_tokens":512,`,
                replies: [],
                sends: 0,
                stopped: "unreadable request body",
            },
            ...["null", "[]", "7"].map((body) => ({
                forward: {},
                body,
                replies: [],
                sends: 0,
                stopped: "not a JSON object",
            })),
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

    test("a timed-out send is charged its reservation", async () => {
        const double = upstreamDouble([() => new Promise<Response>(() => {})]);
        const mock = new MockProvider({
            forward: config({ fetch: double.send, limits: { ...config().limits, timeoutMs: 50 } }),
        });
        await (await post(await start(mock), firstTurn)).text();
        const report = mock.forwardingReport();
        expect(report.exchanges[0]?.response?.outcome).toBe("ambiguous");
        expect(report.exchanges[0]?.response?.cost_known).toBe(false);
        expect(report.spent_usd).toBe(report.exchanges[0]?.request.reserved_usd ?? -1);
    });

    test("missing or partial usage is charged its reservation, never zero", async () => {
        const noUsage = () =>
            new Response(
                JSON.stringify({
                    type: "message",
                    model: MODEL,
                    stop_reason: "end_turn",
                    content: [],
                }),
                {
                    headers: { "content-type": "application/json" },
                },
            );
        const partial = () =>
            new Response(
                JSON.stringify({
                    type: "message",
                    model: MODEL,
                    stop_reason: "end_turn",
                    content: [],
                    usage: { input_tokens: 40 },
                }),
                { headers: { "content-type": "application/json" } },
            );
        const double = upstreamDouble([noUsage, partial]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        const base = await start(mock);
        await (await post(base, { ...firstTurn, stream: false })).text();
        await (await post(base, { ...firstTurn, stream: false })).text();
        expect(mock.forwardingReport().exchanges[1]?.response?.cost_known).toBe(false);
        const report = mock.forwardingReport();
        const exchange = report.exchanges[0];
        expect(exchange?.response?.cost_known).toBe(false);
        expect(exchange?.response?.cost_usd).toBe(exchange?.request.reserved_usd ?? -1);
        expect(report.spent_usd).toBeGreaterThan(0);
        expect(report.incomplete_reasons).toContain("cost unknown for a send");
    });

    test("a response that is not a Messages message stops the run", async () => {
        const bare = () =>
            new Response(JSON.stringify({ model: MODEL, stop_reason: "end_turn", usage: USAGE }), {
                headers: { "content-type": "application/json" },
            });
        const mock = new MockProvider({ forward: config({ fetch: upstreamDouble([bare]).send }) });
        const base = await start(mock);
        expect((await post(base, { ...firstTurn, stream: false })).status).toBe(200);
        expect(mock.forwardingReport().stopped).toBe("send 0 is not a Messages message");
        expect(mock.forwardingReport().complete).toBe(false);

        const headless = [
            'data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}',
            `data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":${JSON.stringify(USAGE)}}`,
            'data: {"type":"message_stop"}',
        ].join("\n");
        expect(readResponse("text/event-stream", headless).malformed).toBe(
            "is not a Messages message",
        );
        expect(readResponse("text/event-stream", sse([], "end_turn")).malformed).toBeNull();
    });

    test("a response above MAX_RESPONSE_BYTES is cut off, charged its reservation, and stops the run", async () => {
        const chunk = new Uint8Array(1024 * 1024).fill(0x20);
        let sent = 0;
        const stream = new ReadableStream<Uint8Array>({
            pull(controller) {
                if (sent++ < 20) controller.enqueue(chunk);
                else controller.close();
            },
        });
        const double = upstreamDouble([
            () => new Response(stream, { headers: { "content-type": "application/json" } }),
        ]);
        const mock = new MockProvider({ forward: config({ fetch: double.send }) });
        const reply = await post(await start(mock), { ...firstTurn, stream: false });
        const text = await reply.text();
        expect(reply.status).toBe(400);
        expect(text).toContain("ResponseTooLarge");
        const report = mock.forwardingReport();
        expect(report.stopped).toBe("send 0 ended without a response (ResponseTooLarge)");
        const exchange = report.exchanges[0];
        expect(exchange?.response?.outcome).toBe("ambiguous");
        expect(exchange?.response?.body_bytes).toBe(0);
        expect(report.spent_usd).toBe(exchange?.request.reserved_usd ?? -1);
        expect(MAX_RESPONSE_BYTES).toBeGreaterThan(4 * 1024 * 1024);
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
        expect(mock.forwardingReport().complete).toBe(false);
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
            expect(published).not.toContain(mock.inboundKey);
            expect(published).toContain("[redacted]");
            expect(JSON.stringify(mock.requests())).not.toContain(FAKE_KEY);
            expect(JSON.stringify(mock.requests())).not.toContain(mock.inboundKey);
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
        expect(() =>
            publishForwardingReport(mock.forwardingReport(), resolve(import.meta.dir, "new"), "x"),
        ).toThrow("inside the repository");
        expect(existsSync(resolve(import.meta.dir, "new"))).toBe(false);
        const shared = mkdtempSync(join(tmpdir(), "forwarding-shared-"));
        try {
            chmodSync(shared, 0o755);
            expect(() => publishForwardingReport(mock.forwardingReport(), shared, "x")).toThrow(
                "owner-only",
            );
            expect(() => publishForwardingReport(mock.forwardingReport(), shared, "../x")).toThrow(
                "plain file label",
            );
            expect(() =>
                publishForwardingReport(mock.forwardingReport(), `${shared}/../x`, "x"),
            ).toThrow("parent directory");
        } finally {
            rmSync(shared, { recursive: true, force: true });
        }

        const env = createIsolatedEnv();
        try {
            __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                mockProviderURL: "http://127.0.0.1:4321",
                modelId: MODEL,
                modelContextLimit: 200_000,
                modelOutputLimit: 1024,
                mockApiKey: mock.inboundKey,
            });
            const generated = readFileSync(join(env.configDir, "opencode.json"), "utf8");
            expect(generated).toContain(`"${MODEL}"`);
            expect(generated).toContain(mock.inboundKey);
            expect(generated).not.toContain(FAKE_KEY);
            const limit = (
                JSON.parse(generated) as {
                    provider: Record<
                        string,
                        { models: Record<string, { limit: { context: number; output: number } }> }
                    >;
                }
            ).provider["mock-anthropic"]?.models[MODEL]?.limit;
            expect(limit).toEqual({ context: 200_000, output: 1024 });
            expect(statSync(resolve(env.configDir, "..")).mode & 0o777).toBe(0o700);
            expect(statSync(join(env.configDir, "opencode.json")).mode & 0o777).toBe(0o600);
        } finally {
            rmSync(resolve(env.configDir, ".."), { recursive: true, force: true });
        }
    });
});
