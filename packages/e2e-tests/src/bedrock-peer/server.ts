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
import { answer, type Caller, type ConverseRequest, callerOf } from "./answers";
import { encodeEvent } from "./eventstream";
import { expectedSignature, parseAuthorization } from "./sigv4";

export interface BedrockCredentials {
    accessKeyId: string;
    secretAccessKey: string;
    sessionToken?: string;
    region: string;
}

export interface CapturedBedrockRequest {
    transport: "http1" | "h2";
    method: string;
    path: string;
    modelId: string | undefined;
    caller: Caller | undefined;
    accessKeyId: string | undefined;
    signatureValid: boolean;
    sessionToken: string | undefined;
    headerNames: string[];
    userAgent: string;
    status: number;
    systemHead: string;
    lastUser: string;
}

interface Received {
    method: string;
    path: string;
    headers: Record<string, string>;
    body: Buffer;
}

interface Reply {
    status: number;
    headers: Record<string, string>;
    body: Buffer;
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

function converseText(body: unknown): ConverseRequest {
    const record = (body ?? {}) as {
        system?: Array<{ text?: unknown }>;
        messages?: Array<{ role?: unknown; content?: Array<{ text?: unknown }> }>;
    };
    const text = (blocks: Array<{ text?: unknown }> | undefined): string =>
        (blocks ?? [])
            .map((block) => (typeof block.text === "string" ? block.text : ""))
            .filter((part) => part.length > 0)
            .join("\n");
    const messages = record.messages ?? [];
    const lastUser = [...messages].reverse().find((message) => message.role === "user");
    return {
        system: text(record.system),
        messages: messages.map((message) => text(message.content)).join("\n"),
        lastUser: text(lastUser?.content),
    };
}

const errorReply = (status: number, type: string, message: string): Reply => ({
    status,
    headers: { "content-type": "application/json", "x-amzn-errortype": type },
    body: Buffer.from(JSON.stringify({ message })),
});

export class BedrockPeer {
    readonly requests: CapturedBedrockRequest[] = [];
    /** `conversationInputTokens` is the input usage a conversation answer reports, so a scenario can raise context pressure. */
    conversationInputTokens = 10;
    private http1: Server | undefined;
    private h2: Http2Server | undefined;

    constructor(readonly credentials: BedrockCredentials) {}

    get http1Url(): string {
        return `http://127.0.0.1:${(this.http1?.address() as AddressInfo).port}`;
    }

    get h2Url(): string {
        return `http://127.0.0.1:${(this.h2?.address() as AddressInfo).port}`;
    }

    async start(): Promise<void> {
        this.http1 = createHttp1Server((req, res) => {
            const chunks: Buffer[] = [];
            req.on("data", (chunk: Buffer) => chunks.push(chunk));
            req.on("end", () => {
                const reply = this.handle("http1", {
                    method: req.method ?? "",
                    path: req.url ?? "",
                    headers: flatHeaders(req.headers),
                    body: Buffer.concat(chunks),
                });
                res.writeHead(reply.status, reply.headers);
                res.end(reply.body);
            });
        });
        this.h2 = createHttp2Server();
        this.h2.on("stream", (stream: ServerHttp2Stream, headers: Http2Headers) => {
            const chunks: Buffer[] = [];
            stream.on("data", (chunk: Buffer) => chunks.push(chunk));
            stream.on("end", () => {
                const flat = flatHeaders(headers);
                flat.host = flat[":authority"] ?? "";
                const reply = this.handle("h2", {
                    method: flat[":method"] ?? "",
                    path: flat[":path"] ?? "",
                    headers: flat,
                    body: Buffer.concat(chunks),
                });
                stream.respond({ ":status": reply.status, ...reply.headers });
                stream.end(reply.body);
            });
        });
        await Promise.all(
            [this.http1, this.h2].map(
                (server) =>
                    new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve())),
            ),
        );
    }

    async stop(): Promise<void> {
        this.http1?.closeAllConnections();
        await Promise.all(
            [this.http1, this.h2].map(
                (server) =>
                    new Promise<void>((resolve) =>
                        server ? server.close(() => resolve()) : resolve(),
                    ),
            ),
        );
    }

    callers(): Caller[] {
        return this.requests.flatMap((request) =>
            request.status === 200 && request.caller ? [request.caller] : [],
        );
    }

    private handle(transport: "http1" | "h2", received: Received): Reply {
        const route = ROUTE.exec(received.path.split("?", 1)[0] as string);
        const authorization = parseAuthorization(received.headers.authorization);
        const signatureValid =
            authorization !== undefined &&
            authorization.accessKeyId === this.credentials.accessKeyId &&
            authorization.region === this.credentials.region &&
            authorization.service === "bedrock" &&
            (authorization.signedHeaders.includes("host") ||
                authorization.signedHeaders.includes(":authority")) &&
            expectedSignature(received, authorization, this.credentials.secretAccessKey) ===
                authorization.signature;
        const capture: CapturedBedrockRequest = {
            transport,
            method: received.method,
            path: received.path,
            modelId: route ? decodeURIComponent(route[1] as string) : undefined,
            caller: undefined,
            accessKeyId: authorization?.accessKeyId,
            signatureValid,
            sessionToken: received.headers["x-amz-security-token"],
            headerNames: Object.keys(received.headers).sort(),
            userAgent: received.headers["user-agent"] ?? "",
            status: 0,
            systemHead: "",
            lastUser: "",
        };
        this.requests.push(capture);
        const reply = ((): Reply => {
            if (!route || received.method !== "POST") {
                return errorReply(404, "UnknownOperationException", "unknown operation");
            }
            if (!signatureValid) {
                return errorReply(403, "InvalidSignatureException", "signature mismatch");
            }
            let request: ConverseRequest;
            try {
                request = converseText(JSON.parse(received.body.toString("utf8")));
            } catch {
                return errorReply(400, "ValidationException", "malformed body");
            }
            capture.caller = callerOf(request);
            capture.systemHead = request.system.slice(0, 200);
            capture.lastUser = request.lastUser.slice(0, 4_000);
            const text = answer(capture.caller, request);
            const inputTokens =
                capture.caller === "conversation" ? this.conversationInputTokens : 10;
            const usage = { inputTokens, outputTokens: 10, totalTokens: inputTokens + 10 };
            return route[2] === "converse-stream"
                ? streamReply(text, usage)
                : unaryReply(text, usage);
        })();
        capture.status = reply.status;
        return reply;
    }
}

interface Usage {
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
}

function streamReply(text: string, usage: Usage): Reply {
    return {
        status: 200,
        headers: { "content-type": "application/vnd.amazon.eventstream" },
        body: Buffer.concat([
            encodeEvent("messageStart", { role: "assistant" }),
            encodeEvent("contentBlockDelta", { contentBlockIndex: 0, delta: { text } }),
            encodeEvent("contentBlockStop", { contentBlockIndex: 0 }),
            encodeEvent("messageStop", { stopReason: "end_turn" }),
            encodeEvent("metadata", { usage, metrics: { latencyMs: 1 } }),
        ]),
    };
}

function unaryReply(text: string, usage: Usage): Reply {
    return {
        status: 200,
        headers: { "content-type": "application/json" },
        body: Buffer.from(
            JSON.stringify({
                output: { message: { role: "assistant", content: [{ text }] } },
                stopReason: "end_turn",
                usage,
                metrics: { latencyMs: 1 },
            }),
        ),
    };
}
