import { afterEach, describe, expect, test } from "bun:test";
import type {
    CassetteMiss,
    CassetteOracle,
    LookupOutcome,
    OracleRequest,
    RecordedResponse,
} from "./cassette-oracle";
import { CassetteRefused } from "./cassette-oracle";
import { type CassetteSession, MockProvider, type MockResponse } from "./server";

const USAGE = { input_tokens: 10, output_tokens: 2 };
const NAMESPACE = "unit:fresh";
const DIGEST = "d".repeat(64);

/**
 * Stands in for the Rust process with the same observable contract: entries
 * keyed by the body text the mock forwards, consumed in order; the first miss
 * latches and answers every later lookup; `turn` counts lookups; the nearest
 * entry is the first unconsumed one, or the last once all are consumed. It
 * proves what the mock does with hits, misses, and refusals; the digest and
 * admission rules themselves are Rust's tests.
 */
class FakeOracle implements Pick<CassetteOracle, "lookup" | "record"> {
    readonly recorded: Array<{ request: OracleRequest; response: RecordedResponse }> = [];
    readonly lookups: OracleRequest[] = [];
    refuse: Error | null = null;
    private consumed = 0;
    private terminal: CassetteMiss | null = null;

    constructor(
        private readonly entries: Array<{ body_text: string; response: RecordedResponse }>,
    ) {}

    async lookup(_namespace: string, request: OracleRequest): Promise<LookupOutcome> {
        if (this.refuse) throw this.refuse;
        this.lookups.push(request);
        if (this.terminal) return { miss: this.terminal };
        const entry = this.entries[this.consumed];
        if (entry && entry.body_text === request.body_text) {
            this.consumed += 1;
            return { hit: { request_digest: DIGEST, response: entry.response } };
        }
        this.terminal = {
            turn: this.lookups.length - 1,
            class: "ModelRequestChanged",
            request_digest: "e".repeat(64),
            nearest_recorded: this.entries.length > 0 ? DIGEST : null,
        };
        return { miss: this.terminal };
    }

    async record(_namespace: string, request: OracleRequest, response: RecordedResponse) {
        if (this.refuse) throw this.refuse;
        this.recorded.push({ request, response });
        return { request_digest: DIGEST };
    }
}

async function post(baseURL: string, body: unknown, stream = true): Promise<Response> {
    const payload =
        typeof body === "string" ? body : JSON.stringify({ ...(body as object), stream });
    return fetch(`${baseURL}/messages`, {
        method: "POST",
        headers: { "content-type": "application/json", "x-api-key": "test-key-not-real" },
        body: payload,
    });
}

const request = { model: "mock-sonnet", messages: [{ role: "user", content: "hi" }] };

const FRAMES = [
    'event: message_start\ndata: {"type":"message_start","message":{"id":"msg_recorded"}}\n\n',
    'event: message_stop\ndata: {"type":"message_stop"}\n\n',
];

function sse(frames: string[]): RecordedResponse {
    return { status: 200, content_type: "text/event-stream", frames, aborted: false };
}

describe("MockProvider cassette mode", () => {
    const mocks: MockProvider[] = [];
    afterEach(async () => {
        for (const mock of mocks.splice(0)) await mock.stop();
    });

    async function started(
        session?: CassetteSession,
    ): Promise<{ mock: MockProvider; baseURL: string }> {
        const mock = new MockProvider();
        mocks.push(mock);
        const { baseURL } = await mock.start();
        if (session) mock.useCassette(session);
        return { mock, baseURL };
    }

    test("record mode hands every produced response to the oracle before serving it", async () => {
        const oracle = new FakeOracle([]);
        const { mock, baseURL } = await started({ oracle, mode: "record", namespace: NAMESPACE });
        mock.setDefault({ text: "ok", usage: USAGE, abortAfterFrames: 3 });
        const response = await post(baseURL, request);
        expect(response.status).toBe(200);
        const served = await response.text();
        expect(oracle.recorded).toHaveLength(1);
        const [{ request: forwarded, response: produced }] = oracle.recorded;
        // The stream ends before `message_stop`, exactly where the recording says it did.
        expect(served).toBe(produced.frames.join(""));
        expect(served).not.toContain("message_stop");
        expect(forwarded.path).toBe("/messages");
        expect(forwarded.headers["x-api-key"]).toBe("test-key-not-real");
        expect(JSON.parse(forwarded.body_text)).toEqual({ ...request, stream: true });
        expect(produced.aborted).toBe(true);
        expect(produced.frames).toHaveLength(3);
        expect(produced.frames[0]).toStartWith("event: message_start\n");
        expect(mock.scriptedSelectionCount()).toBe(1);
    });

    test("a delayed record-mode response is admitted by the cassette that accepted the request", async () => {
        const accepted = new FakeOracle([]);
        const later = new FakeOracle([]);
        const { mock, baseURL } = await started({
            oracle: accepted,
            mode: "record",
            namespace: NAMESPACE,
        });
        mock.setDefault({ text: "slow", usage: USAGE, delayMs: 80 });
        const pending = post(baseURL, request);
        await Bun.sleep(20);
        // The binding changes while the request sleeps; the in-flight exchange stays with `accepted`.
        mock.useCassette({ oracle: later, mode: "record", namespace: NAMESPACE });
        const response = await pending;
        expect(response.status).toBe(200);
        expect(accepted.recorded).toHaveLength(1);
        expect(later.recorded).toHaveLength(0);
    });

    test("a request whose body is still uploading stays with the cassette bound when it began", async () => {
        const accepted = new FakeOracle([]);
        const later = new FakeOracle([]);
        const { mock, baseURL } = await started({
            oracle: accepted,
            mode: "record",
            namespace: NAMESPACE,
        });
        mock.setDefault({ text: "ok", usage: USAGE });
        const encoder = new TextEncoder();
        const body = new ReadableStream({
            async start(controller) {
                controller.enqueue(encoder.encode('{"model":"mock-sonnet",'));
                await Bun.sleep(80);
                controller.enqueue(encoder.encode('"messages":[]}'));
                controller.close();
            },
        });
        const pending = fetch(`${baseURL}/messages`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body,
            duplex: "half",
        } as RequestInit);
        await Bun.sleep(20);
        mock.useCassette({ oracle: later, mode: "record", namespace: NAMESPACE });
        expect((await pending).status).toBe(200);
        expect(accepted.recorded).toHaveLength(1);
        expect(later.recorded).toHaveLength(0);
    });

    test("a request still uploading across reset() consumes and records into the run it began in", async () => {
        const { mock, baseURL } = await started();
        mock.enqueue({ text: "old run", usage: USAGE });
        const encoder = new TextEncoder();
        const body = new ReadableStream({
            async start(controller) {
                controller.enqueue(encoder.encode('{"model":"mock-sonnet",'));
                await Bun.sleep(80);
                controller.enqueue(encoder.encode('"messages":[]}'));
                controller.close();
            },
        });
        const stale = fetch(`${baseURL}/messages`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body,
            duplex: "half",
        } as RequestInit);
        await Bun.sleep(20);
        mock.reset();
        mock.enqueue({ text: "new run", usage: USAGE });
        expect(await (await stale).text()).toContain("old run");
        // The new run saw nothing of the stale request: no capture, no selection, its queue intact.
        expect(mock.requests()).toHaveLength(0);
        expect(mock.scriptedSelectionCount()).toBe(0);
        const fresh = await post(baseURL, request, false);
        expect(await fresh.text()).toContain("new run");
    });

    test("a completion that lands after reset() writes no miss or refusal into the reset logs", async () => {
        const inner = new FakeOracle([]);
        const slow: CassetteSession["oracle"] = {
            lookup: async (namespace, request) => {
                await Bun.sleep(80);
                return inner.lookup(namespace, request);
            },
            record: (namespace, request, response) => inner.record(namespace, request, response),
        };
        const { mock, baseURL } = await started({
            oracle: slow,
            mode: "replay",
            namespace: NAMESPACE,
        });
        const missed = post(baseURL, request);
        await Bun.sleep(20);
        mock.reset();
        expect((await missed).status).toBe(400);
        expect(mock.cassetteMissLog()).toEqual([]);

        mock.useCassette({ oracle: slow, mode: "replay", namespace: NAMESPACE });
        inner.refuse = new CassetteRefused("WrongNamespace", "");
        const refused = post(baseURL, request);
        await Bun.sleep(20);
        mock.reset();
        expect((await refused).status).toBe(400);
        expect(mock.cassetteRefusalLog()).toEqual([]);
    });

    test("concurrent identical requests are recorded in capture order, not completion order", async () => {
        const oracle = new FakeOracle([]);
        const { mock, baseURL } = await started({ oracle, mode: "record", namespace: NAMESPACE });
        mock.enqueue({ text: "first", usage: USAGE, delayMs: 80 });
        mock.enqueue({ text: "second", usage: USAGE });
        const first = post(baseURL, request, false);
        await Bun.sleep(10);
        const second = post(baseURL, request, false);
        expect((await first).status).toBe(200);
        expect((await second).status).toBe(200);
        const texts = oracle.recorded.map(
            ({ response }) => JSON.parse(response.frames[0]).content[0].text,
        );
        expect(texts).toEqual(["first", "second"]);
    });

    test("useCassette() starts a new log generation, so a pending lookup's miss stays with the old binding", async () => {
        const inner = new FakeOracle([]);
        const slow: CassetteSession["oracle"] = {
            lookup: async (namespace, request) => {
                await Bun.sleep(80);
                return inner.lookup(namespace, request);
            },
            record: (namespace, request, response) => inner.record(namespace, request, response),
        };
        const { mock, baseURL } = await started({
            oracle: slow,
            mode: "replay",
            namespace: NAMESPACE,
        });
        const missed = post(baseURL, request);
        await Bun.sleep(20);
        mock.useCassette({ oracle: new FakeOracle([]), mode: "replay", namespace: NAMESPACE });
        expect((await missed).status).toBe(400);
        expect(mock.cassetteMissLog()).toEqual([]);
    });

    test("a JSON body that is not an object is scripted as an empty object and reaches the oracle as text", async () => {
        const oracle = new FakeOracle([]);
        const { mock, baseURL } = await started({ oracle, mode: "record", namespace: NAMESPACE });
        mock.setDefault({ text: "ok", usage: USAGE });
        const response = await post(baseURL, "null");
        expect(response.status).toBe(200);
        expect(oracle.recorded[0]?.request.body_text).toBe("null");
        expect(mock.lastRequest()?.body).toEqual({});
    });

    test("a mock misconfiguration in record mode is served as a 500 and never recorded", async () => {
        const oracle = new FakeOracle([]);
        const { mock, baseURL } = await started({ oracle, mode: "record", namespace: NAMESPACE });
        // Neither `usage` nor `error`: a script bug, not a provider behavior worth a cassette case.
        mock.setDefault({ text: "no usage" });
        const response = await post(baseURL, request);
        expect(response.status).toBe(500);
        expect(await response.json()).toEqual({
            type: "error",
            error: { type: "mock_error", message: "MockResponse requires `usage` or `error`" },
        });
        // An error status `Response` cannot serve is the same kind of script bug.
        mock.setDefault({ error: { status: 600, type: "overloaded_error", message: "m" } });
        const unservable = await post(baseURL, request);
        expect(unservable.status).toBe(500);
        expect(await unservable.json()).toEqual({
            type: "error",
            error: {
                type: "mock_error",
                message: "MockResponse.error.status must be an integer from 200 to 599",
            },
        });
        expect(oracle.recorded).toHaveLength(0);
        expect(mock.cassetteRefusalLog()).toEqual([]);
    });

    test("a recording refusal serves only the refusal kind and persists nothing", async () => {
        const oracle = new FakeOracle([]);
        oracle.refuse = new CassetteRefused("RedactionRefused", "(Request, SecretDetected)");
        const { mock, baseURL } = await started({ oracle, mode: "record", namespace: NAMESPACE });
        mock.setDefault({ text: "sk-ant-canary", usage: USAGE });
        const response = await post(baseURL, request);
        expect(response.status).toBe(400);
        expect(await response.json()).toEqual({
            type: "error",
            error: { type: "redaction_refused", kind: "RedactionRefused" },
        });
        expect(oracle.recorded).toHaveLength(0);
        expect(mock.cassetteRefusalLog().map((refusal) => refusal.kind)).toEqual([
            "RedactionRefused",
        ]);
    });

    test("replay serves recorded frames byte for byte and never enters the scripted block", async () => {
        const errorFrame: RecordedResponse = {
            status: 529,
            content_type: "application/json",
            frames: ['{"type":"error","error":{"type":"overloaded_error","message":"busy"}}'],
            aborted: false,
        };
        const bodyText = JSON.stringify({ ...request, stream: true });
        const oracle = new FakeOracle([
            { body_text: bodyText, response: sse(FRAMES) },
            { body_text: bodyText, response: errorFrame },
        ]);
        const { mock, baseURL } = await started({ oracle, mode: "replay", namespace: NAMESPACE });
        const sentinel: MockResponse = { text: "SENTINEL", usage: USAGE };
        mock.script([sentinel]);
        mock.setDefault({ text: "DEFAULT", usage: USAGE });
        mock.addMatcher(() => ({ text: "MATCHER", usage: USAGE }));

        const hit = await post(baseURL, request);
        expect(hit.status).toBe(200);
        expect(hit.headers.get("content-type")).toBe("text/event-stream");
        expect(await hit.text()).toBe(FRAMES.join(""));

        const providerError = await post(baseURL, request);
        expect(providerError.status).toBe(529);
        expect(await providerError.text()).toBe(errorFrame.frames[0]);

        const miss = await post(baseURL, { ...request, model: "mock-sonnex" });
        expect(miss.status).toBe(400);
        const body = (await miss.json()) as { error: CassetteMiss & { type: string } };
        expect(body.error).toEqual({
            type: "cassette_miss",
            turn: 2,
            class: "ModelRequestChanged",
            request_digest: "e".repeat(64),
            nearest_recorded: DIGEST,
        });
        // The run stops here: the recorded request is refused with the same terminal.
        const afterMiss = await post(baseURL, request);
        expect(afterMiss.status).toBe(400);
        expect(mock.cassetteMissLog()).toHaveLength(2);
        expect(new Set(mock.cassetteMissLog().map((m) => JSON.stringify(m))).size).toBe(1);

        expect(mock.scriptedSelectionCount()).toBe(0);
        expect(mock.defaultHits()).toBe(0);
        expect(oracle.lookups).toHaveLength(4);
        for (const text of [FRAMES.join(""), errorFrame.frames[0], JSON.stringify(body)]) {
            expect(text).not.toContain("SENTINEL");
            expect(text).not.toContain("DEFAULT");
            expect(text).not.toContain("MATCHER");
        }
        // `reset()` unbinds the cassette and clears its logs; the scripted block is live again.
        mock.reset();
        mock.addMatcher(() => ({ text: "MATCHER", usage: USAGE }));
        const scripted = await post(baseURL, request, false);
        expect(await scripted.text()).toContain("MATCHER");
        expect(mock.scriptedSelectionCount()).toBe(1);
        expect(mock.cassetteMissLog()).toEqual([]);
        expect(mock.requests()).toHaveLength(1);
    });

    test("a malformed body reaches the oracle as text, never as an empty object", async () => {
        const oracle = new FakeOracle([]);
        const { mock, baseURL } = await started({ oracle, mode: "replay", namespace: NAMESPACE });
        const response = await post(baseURL, "{not json");
        expect(response.status).toBe(400);
        expect(oracle.lookups[0]?.body_text).toBe("{not json");
        expect(mock.lastRequest()?.body).toEqual({});
    });

    test("an oracle failure of any kind is a typed refusal with no message text", async () => {
        const oracle = new FakeOracle([{ body_text: "irrelevant", response: sse(FRAMES) }]);
        const { mock, baseURL } = await started({ oracle, mode: "replay", namespace: NAMESPACE });
        oracle.refuse = new CassetteRefused("WrongNamespace", 'recorded: "a", offered: "b"');
        const refused = await post(baseURL, request);
        expect(refused.status).toBe(400);
        expect(await refused.json()).toEqual({
            type: "error",
            error: { type: "cassette_refused", kind: "WrongNamespace" },
        });
        oracle.refuse = new Error("child exited with secret-bearing stderr: sk-ant-canary");
        const dead = await post(baseURL, request);
        expect(dead.status).toBe(400);
        const text = await dead.text();
        expect(text).not.toContain("sk-ant-canary");
        expect(JSON.parse(text)).toEqual({
            type: "error",
            error: { type: "cassette_refused", kind: "OracleUnavailable" },
        });
        expect(mock.cassetteRefusalLog().map((refusal) => refusal.kind)).toEqual([
            "WrongNamespace",
            "OracleUnavailable",
        ]);
        expect(mock.cassetteMissLog()).toEqual([]);
        expect(mock.scriptedSelectionCount()).toBe(0);
    });

    test("scripted mode is unchanged: streaming frames end in message_stop and JSON mode returns one message", async () => {
        const { mock, baseURL } = await started();
        mock.script([
            {
                content: [
                    { type: "tool_use", id: "toolu_1", name: "glob", input: { pattern: "*" } },
                ],
                stop_reason: "tool_use",
                usage: USAGE,
            },
            { text: "done", usage: USAGE },
        ]);
        const streamed = await (await post(baseURL, request)).text();
        expect(streamed).toContain('"input_json_delta","partial_json":"{\\"pattern\\":\\"*\\"}"');
        expect(streamed).toContain('"stop_reason":"tool_use"');
        expect(streamed).toEndWith('event: message_stop\ndata: {"type":"message_stop"}\n\n');
        const single = (await (await post(baseURL, request, false)).json()) as {
            content: unknown;
            usage: unknown;
        };
        expect(single.content).toEqual([{ type: "text", text: "done" }]);
        expect(single.usage).toEqual({
            input_tokens: 10,
            output_tokens: 2,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        });
        expect(mock.scriptedSelectionCount()).toBe(2);
    });
});
