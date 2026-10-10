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

interface Sink {
    status(code: number, headers: Record<string, string>): void;
    write(chunk: Buffer): void;
    end(): void;
}

interface ConverseBody {
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

function classify(body: ConverseBody): Caller {
    const system = blockText(body.system);
    const messages = (body.messages ?? []).map((m) => blockText(m.content)).join("\n");
    const tools = body.toolConfig?.tools ?? [];
    if (system.includes("title generator")) return "title";
    if (
        system.includes("context summarization assistant") ||
        (/summar/i.test(system) && tools.length === 0)
    ) {
        return "native_compaction";
    }
    if (system.includes("Extract durable project memory")) return "memory_capture";
    if (system.includes("You are a memory classifier")) return "memory_classifier";
    if (system.includes("You are ContextResearcher")) return "context_researcher";
    if (messages.includes("<new_messages>")) return "history_summarizer";
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

export function estimateTokens(bytes: number): number {
    return Math.round(bytes / 3);
}

export interface GatewayOptions {
    arm: string;
    role: "main" | "closure";
    workdir: string;
    dumpDir: string;
    stripTemperature: boolean;
    onCall: (record: CallRecord) => void;
}

export class BedrockGateway {
    turn: Turn | null = null;
    turnKey: string | null = null;
    probeTokens: { answer: string; stale?: string } | null = null;
    readonly records: CallRecord[] = [];
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
            maxAttempts: 3,
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
            req.on("data", (chunk: Buffer) => chunks.push(chunk));
            req.on("end", () => {
                void this.handle(
                    { path: req.url ?? "", body: Buffer.concat(chunks) },
                    {
                        status: (code, headers) => res.writeHead(code, headers),
                        write: (chunk) => res.write(chunk),
                        end: () => res.end(),
                    },
                );
            });
        });
        this.http1.keepAliveTimeout = 120_000;
        this.h2 = createHttp2Server();
        this.h2.on("stream", (stream: ServerHttp2Stream, headers: Http2Headers) => {
            const chunks: Buffer[] = [];
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
        this.records.push(record);
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
        const base: CallBase = {
            invalidToolPairing: orphanToolResult(body),
            ts: Date.now(),
            arm: this.options.arm,
            role: this.options.role,
            caller,
            turn: this.turnKey,
            estimatedInputTokens: estimateTokens(received.body.length),
            requestBytes: received.body.length,
        };
        if (
            this.options.role === "main" &&
            caller === "main" &&
            this.turn &&
            this.turn.kind !== "probe"
        ) {
            const step = this.scriptedStep(body);
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
        if (this.probeTokens && caller === "main") {
            const text = received.body.toString("utf8");
            const messages = body.messages ?? [];
            const lastUserIndex = messages.map((m) => m.role).lastIndexOf("user");
            const lastUser = JSON.stringify(messages[lastUserIndex] ?? {});
            const tokenIn = (haystack: string, token: string | undefined) =>
                token !== undefined && token.length > 0 && haystack.includes(token);
            base.delivered = {
                answer: tokenIn(text, this.probeTokens.answer),
                stale: tokenIn(text, this.probeTokens.stale),
                inLastUser: tokenIn(lastUser, this.probeTokens.answer),
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
        await this.forward(modelId, streaming, body, sink, base, started);
    }

    private scriptedStep(body: ConverseBody): Array<[string, unknown]> | null {
        const turn = this.turn as Turn;
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
        if (extra?.thinking?.type === "enabled") delete extra.thinking;
        const inference = body.inferenceConfig as { temperature?: number } | undefined;
        if (this.options.stripTemperature && inference?.temperature !== undefined) {
            delete inference.temperature;
            this.temperatureStripped++;
        }
        let localRetries = 0;
        const send = async (command: ConverseStreamCommand | ConverseCommand): Promise<any> => {
            for (let attempt = 0; ; attempt++) {
                try {
                    return await this.client.send(command as ConverseStreamCommand);
                } catch (caught) {
                    const code =
                        (caught as { $metadata?: { httpStatusCode?: number } }).$metadata
                            ?.httpStatusCode ?? 0;
                    const retryable =
                        code === 429 ||
                        code === 503 ||
                        code === 500 ||
                        code === 502 ||
                        code === 504;
                    if (!retryable || attempt >= 12) throw caught;
                    this.retries++;
                    localRetries++;
                    await Bun.sleep(
                        Math.min(60_000, 2_000 * 2 ** Math.min(attempt, 5)) * (0.5 + Math.random()),
                    );
                }
            }
        };
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
                    for (const [type, member] of Object.entries(event)) {
                        if (member === undefined) continue;
                        if (firstByteMs === null) firstByteMs = performance.now() - started;
                        if (type === "metadata")
                            Object.assign(usage, (member as { usage?: object }).usage ?? {});
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
        if (status >= 500 && this.dumped < 3) {
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
