import { describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
    BedrockGateway,
    type CallRecord,
    type ConverseBody,
    classify,
    deliveredToken,
    sendWithRetry,
} from "./gateway";

const SUMMARIZER_SYSTEM_PROMPT = readFileSync(
    resolve(
        import.meta.dir,
        "../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt",
    ),
    "utf8",
);

const text = (value: string) => [{ text: value }];

function throttled(): Error {
    return Object.assign(new Error("Too many requests"), {
        name: "ThrottlingException",
        $metadata: { httpStatusCode: 429 },
    });
}

describe("gateway caller classification", () => {
    it("labels a tool-free history summarizer request by its Eidnara identity", () => {
        const body: ConverseBody = {
            system: text(SUMMARIZER_SYSTEM_PROMPT),
            messages: [
                { role: "user", content: text("<new_messages>\n[1] U: hello\n</new_messages>") },
            ],
        };
        expect(classify(body)).toBe("history_summarizer");
    });

    it("labels a tool-free harness summary request as native compaction", () => {
        const body: ConverseBody = {
            system: text("You are a helpful AI assistant tasked with summarizing conversations."),
            messages: [{ role: "user", content: text("Summarize the conversation above.") }],
        };
        expect(classify(body)).toBe("native_compaction");
    });

    it("labels a memory capture request by its system prompt and payload", () => {
        const body: ConverseBody = {
            system: text("Extract durable project memory from the new messages."),
            messages: [{ role: "user", content: text('{"messages":[],"existing_memories":[]}') }],
        };
        expect(classify(body)).toBe("memory_capture");
    });

    it("labels a request that offers the bash tool as the main agent", () => {
        const body: ConverseBody = {
            system: text("You are a coding agent."),
            messages: [{ role: "user", content: text("Run the tests.") }],
            toolConfig: { tools: [{ toolSpec: { name: "bash" } }] },
        };
        expect(classify(body)).toBe("main");
    });
});

describe("gateway upstream retries", () => {
    it("retries throttling until the call succeeds and reports each retry", async () => {
        let calls = 0;
        let retries = 0;
        const value = await sendWithRetry(
            async () => {
                calls++;
                if (calls < 3) throw throttled();
                return "ok";
            },
            {
                deadlineAt: Date.now() + 10_000,
                signal: new AbortController().signal,
                onRetry: () => retries++,
                backoffMs: () => 1,
            },
        );
        expect(value).toBe("ok");
        expect(calls).toBe(3);
        expect(retries).toBe(2);
    });

    it("does not retry a validation error", async () => {
        let calls = 0;
        const validation = Object.assign(new Error("bad"), { $metadata: { httpStatusCode: 400 } });
        await expect(
            sendWithRetry(
                async () => {
                    calls++;
                    throw validation;
                },
                {
                    deadlineAt: Date.now() + 10_000,
                    signal: new AbortController().signal,
                    onRetry: () => undefined,
                    backoffMs: () => 1,
                },
            ),
        ).rejects.toBe(validation);
        expect(calls).toBe(1);
    });

    it("stops retrying once the next backoff would end past the deadline", async () => {
        let calls = 0;
        const started = Date.now();
        await expect(
            sendWithRetry(
                async () => {
                    calls++;
                    throw throttled();
                },
                {
                    deadlineAt: started + 100,
                    signal: new AbortController().signal,
                    onRetry: () => undefined,
                    backoffMs: () => 40,
                },
            ),
        ).rejects.toMatchObject({ name: "ThrottlingException" });
        expect(calls).toBeLessThanOrEqual(3);
        expect(Date.now() - started).toBeLessThan(150);
    });

    it("abandons a pending backoff when the caller disconnects", async () => {
        const controller = new AbortController();
        const started = Date.now();
        setTimeout(() => controller.abort(), 20);
        await expect(
            sendWithRetry(
                async () => {
                    throw throttled();
                },
                {
                    deadlineAt: started + 60_000,
                    signal: controller.signal,
                    onRetry: () => undefined,
                    backoffMs: () => 30_000,
                },
            ),
        ).rejects.toThrow();
        expect(Date.now() - started).toBeLessThan(1_000);
    });
});

describe("forwarding a stream", () => {
    it("records a stream with metadata but no messageStop as an incomplete call", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        (gateway as unknown as { client: { send: unknown } }).client.send = async () => ({
            stream: (async function* () {
                yield { messageStart: { role: "assistant" } };
                yield { metadata: { usage: { inputTokens: 1, outputTokens: 1 } } };
            })(),
        });
        const sink = {
            status: () => {},
            write: () => {},
            end: () => {},
            signal: new AbortController().signal,
        };
        const base = {
            ts: Date.now(),
            arm: "test",
            role: "main" as const,
            caller: "main" as const,
            turn: null,
            estimatedInputTokens: 0,
            requestBytes: 0,
        };
        try {
            await (
                gateway as unknown as { forward: (...args: unknown[]) => Promise<void> }
            ).forward(
                "model",
                true,
                { messages: [] },
                sink,
                base,
                performance.now(),
                Date.now() + 10_000,
            );
            expect(records[0]?.status).toBe(502);
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });

    it("records a stream that ends before its metadata as an incomplete call", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        (gateway as unknown as { client: { send: unknown } }).client.send = async () => ({
            stream: (async function* () {
                yield { messageStart: { role: "assistant" } };
                yield { messageStop: { stopReason: "end_turn" } };
            })(),
        });
        const sink = {
            status: () => {},
            write: () => {},
            end: () => {},
            signal: new AbortController().signal,
        };
        const base = {
            ts: Date.now(),
            arm: "test",
            role: "main" as const,
            caller: "main" as const,
            turn: null,
            estimatedInputTokens: 0,
            requestBytes: 0,
        };
        try {
            await (
                gateway as unknown as { forward: (...args: unknown[]) => Promise<void> }
            ).forward(
                "model",
                true,
                { messages: [] },
                sink,
                base,
                performance.now(),
                Date.now() + 10_000,
            );
            expect(records[0]?.status).toBe(502);
            expect(records[0]?.error).toContain("metadata");
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });

    it("records a stream that ends without messageStop as an incomplete call", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        (gateway as unknown as { client: { send: unknown } }).client.send = async () => ({
            stream: (async function* () {
                yield { messageStart: { role: "assistant" } };
                yield { contentBlockDelta: { contentBlockIndex: 0, delta: { text: "partial" } } };
            })(),
        });
        const sink = {
            status: () => {},
            write: () => {},
            end: () => {},
            signal: new AbortController().signal,
        };
        const base = {
            ts: Date.now(),
            arm: "test",
            role: "main" as const,
            caller: "main" as const,
            turn: null,
            estimatedInputTokens: 0,
            requestBytes: 0,
        };
        try {
            await (
                gateway as unknown as { forward: (...args: unknown[]) => Promise<void> }
            ).forward(
                "model",
                true,
                { messages: [] },
                sink,
                base,
                performance.now(),
                Date.now() + 10_000,
            );
            expect(records[0]?.status).toBe(502);
            expect(records[0]?.error).toContain("messageStop and metadata");
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });

    it("records a disconnect that lands after the last event as a failed call", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        // The harness drops the connection after the only event, before the provider ends the
        // stream with messageStop.
        const disconnected = new AbortController();
        (gateway as unknown as { client: { send: unknown } }).client.send = async () => ({
            stream: (async function* () {
                yield { messageStart: { role: "assistant" } };
                disconnected.abort();
            })(),
        });
        const sink = {
            status: () => {},
            write: () => {},
            end: () => {},
            signal: disconnected.signal,
        };
        const base = {
            ts: Date.now(),
            arm: "test",
            role: "main" as const,
            caller: "main" as const,
            turn: null,
            estimatedInputTokens: 0,
            requestBytes: 0,
        };
        try {
            await (
                gateway as unknown as { forward: (...args: unknown[]) => Promise<void> }
            ).forward(
                "model",
                true,
                { messages: [] },
                sink,
                base,
                performance.now(),
                Date.now() + 10_000,
            );
            expect(records[0]?.status).toBe(499);
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });

    it("records a stream the harness disconnected from as a failed call", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        // The harness drops the connection after the first event reaches it.
        const disconnected = new AbortController();
        (gateway as unknown as { client: { send: unknown } }).client.send = async () => ({
            stream: (async function* () {
                yield { messageStart: { role: "assistant" } };
                disconnected.abort();
                yield { contentBlockDelta: { contentBlockIndex: 0, delta: { text: "hi" } } };
            })(),
        });
        const sink = {
            status: () => {},
            write: () => {},
            end: () => {},
            signal: disconnected.signal,
        };
        const base = {
            ts: Date.now(),
            arm: "test",
            role: "main" as const,
            caller: "main" as const,
            turn: null,
            estimatedInputTokens: 0,
            requestBytes: 0,
        };
        try {
            await (
                gateway as unknown as { forward: (...args: unknown[]) => Promise<void> }
            ).forward(
                "model",
                true,
                { messages: [] },
                sink,
                base,
                performance.now(),
                Date.now() + 10_000,
            );
            expect(records[0]?.status).toBe(499);
            expect(records[0]?.error).toContain("disconnected");
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });

    it("records an exception event the stream carries as the call's failure", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        const events = [
            { messageStart: { role: "assistant" } },
            { throttlingException: { message: "Too many requests" } },
        ];
        (gateway as unknown as { client: { send: unknown } }).client.send = async () => ({
            stream: (async function* () {
                for (const event of events) yield event;
            })(),
        });
        const written: Buffer[] = [];
        const sink = {
            status: () => {},
            write: (chunk: Buffer) => written.push(chunk),
            end: () => {},
            signal: new AbortController().signal,
        };
        const base = {
            ts: Date.now(),
            arm: "test",
            role: "main" as const,
            caller: "main" as const,
            turn: null,
            estimatedInputTokens: 0,
            requestBytes: 0,
        };
        try {
            await (
                gateway as unknown as {
                    forward: (...args: unknown[]) => Promise<void>;
                }
            ).forward(
                "model",
                true,
                { messages: [] },
                sink,
                base,
                performance.now(),
                Date.now() + 10_000,
            );
            expect(records).toHaveLength(1);
            expect(records[0]?.status).toBe(429);
            expect(records[0]?.error).toContain("throttlingException");
            // The harness still receives the event the provider sent.
            expect(written.length).toBe(2);
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });
});

describe("gateway upstream retries until the deadline", () => {
    it("keeps retrying a throttled call while the turn's deadline holds", async () => {
        let calls = 0;
        const value = await sendWithRetry(
            async () => {
                calls++;
                if (calls < 20) throw throttled();
                return "ok";
            },
            {
                deadlineAt: Date.now() + 10_000,
                signal: new AbortController().signal,
                onRetry: () => {},
                backoffMs: () => 1,
            },
        );
        expect(value).toBe("ok");
        expect(calls).toBe(20);
    });
});

describe("delivery detection", () => {
    it("uses the grader's token boundaries", () => {
        expect(deliveredToken("the value is 27196.", "27196")).toBe(true);
        // The same digits inside another fact's prefixed value are a different token.
        expect(deliveredToken("incident INC-27196 was closed", "27196")).toBe(false);
        expect(deliveredToken("anything", undefined)).toBe(false);
    });
});

describe("gateway rejections", () => {
    it("records a request on an unknown route and a malformed body as failed calls", async () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        const records: CallRecord[] = [];
        const gateway = new BedrockGateway({
            arm: "test",
            role: "main",
            workdir: dumpDir,
            dumpDir,
            stripTemperature: false,
            onCall: (record) => records.push(record),
        });
        const sink = () => ({
            status: () => {},
            write: () => {},
            end: () => {},
            signal: new AbortController().signal,
        });
        const handle = (path: string, body: string) =>
            (gateway as unknown as { handle: (r: unknown, s: unknown) => Promise<void> }).handle(
                { path, body: Buffer.from(body) },
                sink(),
            );
        try {
            await handle("/model/x/unknown-op", "{}");
            await handle("/model/x/converse", "{not json");
            expect(records.map((r) => [r.caller, r.status])).toEqual([
                ["rejected", 404],
                ["rejected", 400],
            ]);
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });
});

describe("forwarded-call attribution", () => {
    it("counts forwarded calls by the turn they were recorded under", () => {
        const dumpDir = mkdtempSync(join(tmpdir(), "ab-gateway-"));
        try {
            const gateway = new BedrockGateway({
                arm: "test",
                role: "main",
                workdir: dumpDir,
                dumpDir,
                stripTemperature: false,
                onCall: () => {},
            });
            const record = (turn: string | null, caller: "main" | "main_scripted") =>
                (gateway as unknown as { record: (r: Partial<CallRecord>) => void }).record({
                    ts: 0,
                    arm: "test",
                    role: "main",
                    caller,
                    turn,
                    status: 200,
                    ms: 1,
                    firstByteMs: null,
                    inputTokens: 0,
                    outputTokens: 0,
                    cacheReadTokens: 0,
                    cacheWriteTokens: 0,
                    estimatedInputTokens: 0,
                    requestBytes: 0,
                });
            record("0:1", "main");
            record("0:1", "main_scripted");
            // A call that began under turn 0:1 and finished during 0:2 still belongs to 0:1.
            record("0:1", "main");
            expect(gateway.forwardedCallsFor("0:1")).toBe(2);
            expect(gateway.forwardedCallsFor("0:2")).toBe(0);
        } finally {
            rmSync(dumpDir, { recursive: true, force: true });
        }
    });
});
