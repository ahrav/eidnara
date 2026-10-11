import { appendFileSync } from "node:fs";
import {
    createServer as createHttp1Server,
    type IncomingHttpHeaders,
    type Server,
} from "node:http";
import {
    createServer as createHttp2Server,
    type IncomingHttpHeaders as Http2Headers,
    type Http2Server,
    type ServerHttp2Stream,
} from "node:http2";
import type { AddressInfo } from "node:net";
import { join } from "node:path";
import {
    BedrockRuntimeClient,
    ConverseCommand,
    ConverseStreamCommand,
} from "@aws-sdk/client-bedrock-runtime";
import { callerOf } from "../bedrock-peer/answers";
import { encodeEvent } from "../bedrock-peer/eventstream";
import type { Turn } from "./world";

export type Caller =
    | "main"
    | "main_scripted"
    | "native_compaction"
    | "title"
    | "history_summarizer"
    | "memory_capture"
    | "memory_classifier"
    | "context_researcher"
    | "other";

export interface CallRecord {
    ts: number;
    arm: string;
    role: "main" | "closure";
    caller: Caller;
    turn: string | null;
    status: number;
    ms: number;
    firstByteMs: number | null;
    inputTokens: number;
    outputTokens: number;
    cacheReadTokens: number;
    cacheWriteTokens: number;
    estimatedInputTokens: number;
    requestBytes: number;
    retries?: number;
    delivered?: { answer: boolean; stale: boolean; inLastUser: boolean };
    invalidToolPairing?: boolean;
    toolsUsed?: string[];
    error?: string;
}

type CallBase = Omit<
    CallRecord,
    | "status"
    | "ms"
    | "firstByteMs"
    | "inputTokens"
    | "outputTokens"
    | "cacheReadTokens"
    | "cacheWriteTokens"
>;

interface Received {
    path: string;
    body: Buffer;
}

/** HTTP status a ConverseStream exception member would have carried as a rejected request. */
const STREAM_EXCEPTION_STATUS: Record<string, number> = {
    throttlingException: 429,
    validationException: 400,
    serviceUnavailableException: 503,
    internalServerException: 500,
    modelStreamErrorException: 500,
};

interface Sink {
    status(code: number, headers: Record<string, string>): void;
    write(chunk: Buffer): void;
    end(): void;
    signal: AbortSignal;
}

export interface ConverseBody {
    system?: Array<Record<string, unknown>>;
    messages?: Array<{ role: string; content: Array<Record<string, unknown>> }>;
    toolConfig?: {
        tools?: Array<{
            toolSpec?: {
                name: string;
                inputSchema?: { json?: { properties?: Record<string, unknown> } };
            };
        }>;
    };
    [field: string]: unknown;
}

const ROUTE = /^\/model\/([^/]+)\/(converse-stream|converse)$/;

function flatHeaders(headers: IncomingHttpHeaders | Http2Headers): Record<string, string> {
    const flat: Record<string, string> = {};
    for (const [name, value] of Object.entries(headers)) {
        if (value === undefined) continue;
        flat[name.toLowerCase()] = Array.isArray(value) ? value.join(",") : String(value);
    }
    return flat;
}

function blockText(blocks: Array<Record<string, unknown>> | undefined): string {
    return (blocks ?? [])
        .map((block) => (typeof block.text === "string" ? block.text : ""))
        .filter((text) => text.length > 0)
        .join("\n");
}

export function classify(body: ConverseBody): Caller {
    const system = blockText(body.system);
    const messages = body.messages ?? [];
    const lastUser = [...messages].reverse().find((m) => m.role === "user");
    const eidnara = callerOf({
        system,
        messages: messages.map((m) => blockText(m.content)).join("\n"),
        lastUser: blockText(lastUser?.content),
    });
    if (eidnara !== "conversation") return eidnara;
    const tools = body.toolConfig?.tools ?? [];
    if (system.includes("title generator")) return "title";
    if (
        system.includes("context summarization assistant") ||
        (/summar/i.test(system) && tools.length === 0)
    ) {
        return "native_compaction";
    }
    if (tools.some((tool) => tool.toolSpec?.name === "bash")) return "main";
    return "other";
}

function orphanToolResult(body: ConverseBody): boolean {
    const messages = body.messages ?? [];
    for (let i = 0; i < messages.length; i++) {
        const m = messages[i] as { role: string; content: Array<Record<string, unknown>> };
        const results = m.content.filter((b) => b.toolResult !== undefined).length;
        if (results === 0) continue;
        const prev = messages[i - 1];
        const uses =
            prev?.role === "assistant"
                ? prev.content.filter((b) => b.toolUse !== undefined).length
                : 0;
        if (results > uses) return true;
    }
    return false;
}

export interface RetryOptions {
    /** Epoch-millisecond deadline; retries require their backoff to end strictly before `deadlineAt`. */
    deadlineAt: number;
    signal: AbortSignal;
    onRetry: () => void;
    backoffMs?: (attempt: number) => number;
}

const MAX_RETRIES = 12;

const RETRYABLE_STATUS = new Set([429, 500, 502, 503, 504]);

const TRANSIENT_NETWORK_CODES = new Set(["ECONNRESET", "ECONNREFUSED", "EPIPE", "ETIMEDOUT"]);

const defaultBackoffMs = (attempt: number): number =>
    Math.min(60_000, 2_000 * 2 ** Math.min(attempt, 5)) * (0.5 + Math.random());

function retryable(caught: unknown): boolean {
    const failure = caught as {
        $metadata?: { httpStatusCode?: number };
        $retryable?: unknown;
        code?: string;
    };
    return (
        RETRYABLE_STATUS.has(failure.$metadata?.httpStatusCode ?? 0) ||
        Boolean(failure.$retryable) ||
        TRANSIENT_NETWORK_CODES.has(failure.code ?? "")
    );
}

function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
    return new Promise((resolve, reject) => {
        const onAbort = () => {
            clearTimeout(timer);
            reject(signal.reason);
        };
        const timer = setTimeout(() => {
            signal.removeEventListener("abort", onAbort);
            resolve();
        }, ms);
        signal.addEventListener("abort", onAbort, { once: true });
    });
}

export async function sendWithRetry<T>(send: () => Promise<T>, opts: RetryOptions): Promise<T> {
    const backoffMs = opts.backoffMs ?? defaultBackoffMs;
    for (let attempt = 0; ; attempt++) {
        opts.signal.throwIfAborted();
        try {
            return await send();
        } catch (caught) {
            if (opts.signal.aborted || !retryable(caught) || attempt >= MAX_RETRIES) throw caught;
            const delay = backoffMs(attempt);
            if (Date.now() + delay >= opts.deadlineAt) throw caught;
            opts.onRetry();
            await abortableSleep(delay, opts.signal);
        }
    }
}

/**
 * Across 978 forwarded requests of the M-tier run, Converse bodies held 2.38 to 2.82 bytes per
 * Bedrock input token (5th to 95th percentile), so 2.4 bytes per token keeps the estimate at or
 * above the provider's count for nearly every request.
 */
export function estimateTokens(bytes: number): number {
    return Math.round(bytes / 2.4);
}

export interface TurnScope {
    key: string;
    /** The scripted turn the main gateway answers; the closure gateway's scope carries `null`. */
    turn: Turn | null;
    probe: { answer: string; stale?: string } | null;
    /** Epoch milliseconds when the harness stops waiting for this turn; `Infinity` for closure work. */
    deadlineAt: number;
}

const REQUEST_DEADLINE_MS = 900_000;

export interface GatewayOptions {
    arm: string;
    role: "main" | "closure";
    workdir: string;
    dumpDir: string;
    stripTemperature: boolean;
    /** Refuses a request whose estimated input exceeds this many tokens, as a model with that window does. */
    windowTokens?: number;
    onCall: (record: CallRecord) => void;
}

export class BedrockGateway {
    scope: TurnScope | null = null;
    forwardedCalls = 0;
    scriptMismatches = 0;
    private http1: Server | undefined;
    private h2: Http2Server | undefined;
    private readonly client: BedrockRuntimeClient;
    private toolSeq = 0;
    private dumped = 0;
    retries = 0;
    temperatureStripped = 0;

    constructor(private readonly options: GatewayOptions) {
        this.client = new BedrockRuntimeClient({
            region: process.env.AWS_REGION ?? "us-west-2",
            maxAttempts: 1,
        });
    }

    get http1Url(): string {
        if (!this.http1) throw new Error("gateway not started");
        return `http://127.0.0.1:${(this.http1.address() as AddressInfo).port}`;
    }

    get h2Url(): string {
        if (!this.h2) throw new Error("gateway not started");
        return `http://127.0.0.1:${(this.h2.address() as AddressInfo).port}`;
    }

    async start(): Promise<void> {
        this.http1 = createHttp1Server((req, res) => {
            const chunks: Buffer[] = [];
            const disconnected = new AbortController();
            res.on("close", () => {
                if (!res.writableFinished) disconnected.abort();
            });
            req.on("data", (chunk: Buffer) => chunks.push(chunk));
            req.on("end", () => {
                void this.handle(
                    { path: req.url ?? "", body: Buffer.concat(chunks) },
                    {
                        status: (code, headers) => res.writeHead(code, headers),
                        write: (chunk) => res.write(chunk),
                        end: () => res.end(),
                        signal: disconnected.signal,
                    },
                );
            });
        });
        this.http1.keepAliveTimeout = 120_000;
        this.h2 = createHttp2Server();
        this.h2.on("stream", (stream: ServerHttp2Stream, headers: Http2Headers) => {
            const chunks: Buffer[] = [];
            const disconnected = new AbortController();
            stream.on("close", () => {
                if (!stream.writableFinished) disconnected.abort();
            });
            stream.on("data", (chunk: Buffer) => chunks.push(chunk));
            stream.on("error", () => undefined);
            stream.on("end", () => {
                const flat = flatHeaders(headers);
                void this.handle(
                    { path: flat[":path"] ?? "", body: Buffer.concat(chunks) },
                    {
                        status: (code, h) => {
                            if (!stream.destroyed && !stream.headersSent)
                                stream.respond({ ":status": code, ...h });
                        },
                        write: (chunk) => {
                            if (!stream.destroyed) stream.write(chunk);
                        },
                        end: () => {
                            if (!stream.destroyed) stream.end();
                        },
                        signal: disconnected.signal,
                    },
                );
            });
        });
        await Promise.all(
            [this.http1, this.h2].map(
                (server) =>
                    new Promise<void>((done) => server.listen(0, "127.0.0.1", () => done())),
            ),
        );
    }

    async stop(): Promise<void> {
        this.http1?.closeAllConnections();
        await Promise.all(
            [this.http1, this.h2].map(
                (server) =>
                    new Promise<void>((done) => (server ? server.close(() => done()) : done())),
            ),
        );
    }

    private record(record: CallRecord): void {
        if (record.caller !== "main_scripted") this.forwardedCalls++;
        this.options.onCall(record);
    }

    private async handle(received: Received, sink: Sink): Promise<void> {
        const started = performance.now();
        const route = ROUTE.exec(received.path.split("?", 1)[0] as string);
        if (!route) {
            sink.status(404, {
                "content-type": "application/json",
                "x-amzn-errortype": "UnknownOperationException",
            });
            sink.write(Buffer.from('{"message":"unknown operation"}'));
            sink.end();
            return;
        }
        const modelId = decodeURIComponent(route[1] as string);
        const streaming = route[2] === "converse-stream";
        let body: ConverseBody;
        try {
            body = JSON.parse(received.body.toString("utf8")) as ConverseBody;
        } catch {
            sink.status(400, {
                "content-type": "application/json",
                "x-amzn-errortype": "ValidationException",
            });
            sink.write(Buffer.from('{"message":"malformed body"}'));
            sink.end();
            return;
        }
        const caller = classify(body);
        const scope = this.scope;
        const base: CallBase = {
            invalidToolPairing: orphanToolResult(body),
            ts: Date.now(),
            arm: this.options.arm,
            role: this.options.role,
            caller,
            turn: scope?.key ?? null,
            estimatedInputTokens: estimateTokens(received.body.length),
            requestBytes: received.body.length,
        };
        // Scripted turns report the same estimate as their usage, so a harness compacts on the
        // count this gate refuses at.
        const window = this.options.windowTokens;
        const projected = base.estimatedInputTokens;
        if (window !== undefined && projected > window) {
            const message = `prompt is too long: ${projected} tokens > ${window} maximum`;
            sink.status(400, {
                "content-type": "application/json",
                "x-amzn-errortype": "ValidationException",
            });
            sink.write(Buffer.from(JSON.stringify({ message })));
            sink.end();
            this.record({
                ...base,
                status: 400,
                ms: performance.now() - started,
                firstByteMs: null,
                inputTokens: 0,
                outputTokens: 0,
                cacheReadTokens: 0,
                cacheWriteTokens: 0,
                error: `ValidationException: ${message}`,
            });
            return;
        }
        const turn = scope?.turn;
        if (this.options.role === "main" && caller === "main" && turn && turn.kind !== "probe") {
            const step = this.scriptedStep(body, turn);
            if (step) {
                const usage = {
                    inputTokens: base.estimatedInputTokens,
                    outputTokens: 60,
                    totalTokens: base.estimatedInputTokens + 60,
                };
                sink.status(200, { "content-type": "application/vnd.amazon.eventstream" });
                sink.write(Buffer.concat(step.map(([type, member]) => encodeEvent(type, member))));
                sink.write(encodeEvent("metadata", { usage, metrics: { latencyMs: 1 } }));
                sink.end();
                this.record({
                    ...base,
                    caller: "main_scripted",
                    status: 200,
                    ms: performance.now() - started,
                    firstByteMs: 0,
                    inputTokens: 0,
                    outputTokens: 0,
                    cacheReadTokens: 0,
                    cacheWriteTokens: 0,
                });
                return;
            }
            this.scriptMismatches++;
        }
        const probe = scope?.probe;
        if (probe && caller === "main") {
            const text = received.body.toString("utf8");
            const messages = body.messages ?? [];
            const lastUserIndex = messages.map((m) => m.role).lastIndexOf("user");
            const lastUser = JSON.stringify(messages[lastUserIndex] ?? {});
            const tokenIn = (haystack: string, token: string | undefined) =>
                token !== undefined && token.length > 0 && haystack.includes(token);
            base.delivered = {
                answer: tokenIn(text, probe.answer),
                stale: tokenIn(text, probe.stale),
                inLastUser: tokenIn(lastUser, probe.answer),
            };
            let anchor = -1;
            for (let i = messages.length - 1; i >= 0; i--) {
                const m = messages[i] as { role: string; content: Array<Record<string, unknown>> };
                if (
                    m.role === "user" &&
                    m.content.some((b) => typeof b.text === "string") &&
                    !m.content.some((b) => b.toolResult !== undefined)
                ) {
                    anchor = i;
                    break;
                }
            }
            base.toolsUsed = messages
                .slice(anchor + 1)
                .flatMap((m) => m.content)
                .flatMap((b) => (b.toolUse ? [String((b.toolUse as { name?: string }).name)] : []));
        }
        const deadlineAt = Math.min(
            scope?.deadlineAt ?? Number.POSITIVE_INFINITY,
            Date.now() + REQUEST_DEADLINE_MS,
        );
        await this.forward(modelId, streaming, body, sink, base, started, deadlineAt);
    }

    private scriptedStep(body: ConverseBody, turn: Turn): Array<[string, unknown]> | null {
        const messages = body.messages ?? [];
        let anchor = -1;
        for (let i = messages.length - 1; i >= 0; i--) {
            const m = messages[i] as { role: string; content: Array<Record<string, unknown>> };
            if (m.role !== "user") continue;
            const hasText = m.content.some(
                (b) => typeof b.text === "string" && (b.text as string).length > 0,
            );
            const hasResult = m.content.some((b) => b.toolResult !== undefined);
            if (hasText && !hasResult) {
                anchor = i;
                break;
            }
        }
        if (anchor < 0) return null;
        const anchorText = blockText(messages[anchor]?.content);
        if (
            !anchorText.includes(turn.user.slice(0, 80)) ||
            anchorText.length > turn.user.length + 6000
        )
            return null;
        const done = messages.slice(anchor + 1).filter((m) => m.role === "assistant").length;
        const step = turn.steps[done] ?? { kind: "text" as const, text: "Done." };
        if (step.kind === "text") {
            return [
                ["messageStart", { role: "assistant" }],
                ["contentBlockDelta", { contentBlockIndex: 0, delta: { text: step.text } }],
                ["contentBlockStop", { contentBlockIndex: 0 }],
                ["messageStop", { stopReason: "end_turn" }],
            ];
        }
        const spec = (body.toolConfig?.tools ?? []).find(
            (tool) => tool.toolSpec?.name === step.tool,
        )?.toolSpec;
        if (!spec) return null;
        const props = spec.inputSchema?.json?.properties ?? {};
        const input: Record<string, unknown> = {};
        if (step.tool === "read") {
            if ("filePath" in props) input.filePath = join(this.options.workdir, step.path);
            else input.path = step.path;
        } else {
            input.command = step.command;
            if ("description" in props) input.description = step.description;
        }
        const toolUseId = `tooluse_ab${(++this.toolSeq).toString(36)}${Date.now().toString(36)}`;
        return [
            ["messageStart", { role: "assistant" }],
            [
                "contentBlockStart",
                { contentBlockIndex: 0, start: { toolUse: { toolUseId, name: step.tool } } },
            ],
            [
                "contentBlockDelta",
                { contentBlockIndex: 0, delta: { toolUse: { input: JSON.stringify(input) } } },
            ],
            ["contentBlockStop", { contentBlockIndex: 0 }],
            ["messageStop", { stopReason: "tool_use" }],
        ];
    }

    private async forward(
        modelId: string,
        streaming: boolean,
        body: ConverseBody,
        sink: Sink,
        base: CallBase,
        started: number,
        deadlineAt: number,
    ): Promise<void> {
        const usage = {
            inputTokens: 0,
            outputTokens: 0,
            cacheReadInputTokens: 0,
            cacheWriteInputTokens: 0,
        };
        let firstByteMs: number | null = null;
        let status = 200;
        let error: string | undefined;
        let headersSent = false;
        const extra = body.additionalModelRequestFields as
            | { thinking?: { type?: string } }
            | undefined;
        // Every arm forwards without the harness's thinking field, so the `on` and `onraw` arms
        // differ by the temperature alone.
        if (extra?.thinking?.type === "enabled") delete extra.thinking;
        const inference = body.inferenceConfig as { temperature?: number } | undefined;
        if (this.options.stripTemperature && inference?.temperature !== undefined) {
            delete inference.temperature;
            this.temperatureStripped++;
        }
        let localRetries = 0;
        const send = (command: ConverseStreamCommand | ConverseCommand): Promise<any> =>
            sendWithRetry(
                () =>
                    this.client.send(command as ConverseStreamCommand, {
                        abortSignal: sink.signal,
                    }),
                {
                    deadlineAt,
                    signal: sink.signal,
                    onRetry: () => {
                        this.retries++;
                        localRetries++;
                    },
                },
            );
        try {
            if (streaming) {
                const response = await send(
                    new ConverseStreamCommand({ ...body, modelId } as never),
                );
                sink.status(200, { "content-type": "application/vnd.amazon.eventstream" });
                headersSent = true;
                for await (const event of response.stream as AsyncIterable<
                    Record<string, unknown>
                >) {
                    if (sink.signal.aborted) {
                        // The harness gave up on this response, so the call ended short of
                        // the provider's final event.
                        status = 499;
                        error = "client disconnected before the stream ended";
                        break;
                    }
                    for (const [type, member] of Object.entries(event)) {
                        if (member === undefined) continue;
                        if (firstByteMs === null) firstByteMs = performance.now() - started;
                        if (type === "metadata")
                            Object.assign(usage, (member as { usage?: object }).usage ?? {});
                        // Bedrock reports a failure after the response began as a stream member,
                        // so the record carries it as the call's outcome while the harness
                        // still receives the event.
                        if (type.endsWith("Exception")) {
                            status = STREAM_EXCEPTION_STATUS[type] ?? 500;
                            error = `${type}: ${(member as { message?: string }).message ?? ""}`;
                        }
                        sink.write(encodeEvent(type, member));
                    }
                }
            } else {
                const response = await send(new ConverseCommand({ ...body, modelId } as never));
                Object.assign(usage, response.usage ?? {});
                const { $metadata: _metadata, ...rest } = response;
                sink.status(200, { "content-type": "application/json" });
                headersSent = true;
                sink.write(Buffer.from(JSON.stringify(rest)));
            }
        } catch (caught) {
            const failure = caught as {
                name?: string;
                message?: string;
                $metadata?: { httpStatusCode?: number };
            };
            status = failure.$metadata?.httpStatusCode ?? 500;
            error = `${failure.name}: ${failure.message}`;
            if (!headersSent) {
                sink.status(status, {
                    "content-type": "application/json",
                    "x-amzn-errortype": failure.name ?? "InternalServerException",
                });
                sink.write(
                    Buffer.from(JSON.stringify({ message: failure.message ?? "gateway error" })),
                );
            }
        }
        sink.end();
        // A provider refusal of the request's shape (400) is the evidence a harness or Eidnara
        // defect leaves, so the first few of each class are kept with their bodies.
        if (status >= 400 && status !== 429 && this.dumped < 8) {
            this.dumped++;
            appendFileSync(
                join(this.options.dumpDir, "failed-requests.jsonl"),
                `${JSON.stringify({ modelId, error, body })}\n`,
            );
        }
        this.record({
            ...base,
            status,
            ms: performance.now() - started,
            firstByteMs,
            inputTokens: usage.inputTokens ?? 0,
            outputTokens: usage.outputTokens ?? 0,
            cacheReadTokens: usage.cacheReadInputTokens ?? 0,
            cacheWriteTokens: usage.cacheWriteInputTokens ?? 0,
            retries: localRetries,
            ...(error ? { error } : {}),
        });
    }
}
