/**
 * The server emulates Anthropic's Messages API.
 *
 * The server accepts POST requests at `/messages`, matching `@ai-sdk/anthropic`'s `${baseURL}/messages` path.
 * The server captures each request body and returns scripted responses.
 * The mock controls input, output, cache-read, and cache-write token counts.
 *
 * The server supports Anthropic Messages SSE streaming and single-shot JSON responses.
 *
 * In cassette mode every request goes to the Rust oracle. Replay serves recorded frames or a
 * typed `cassette_miss` and never enters the scripted-selection block; record serves the script
 * only after the oracle admits the exchange. Misses and refusals are HTTP 400 because the AI SDK
 * retries 408, 409, 429, and 5xx.
 */

import {
    type CassetteMiss,
    type CassetteMode,
    type CassetteOracle,
    CassetteRefused,
    type OracleRequest,
    type RecordedResponse,
} from "./cassette-oracle";

export interface MockUsage {
    input_tokens: number;
    output_tokens: number;
    cache_read_input_tokens?: number;
    cache_creation_input_tokens?: number;
}

export interface MockResponse {
    /** The mock converts `text` into an Anthropic content array. */
    text?: string;
    /** `content` overrides `text` for tool calls or multiple content blocks. */
    content?: unknown[];
    stop_reason?: "end_turn" | "tool_use" | "max_tokens" | "stop_sequence";
    /**
     * Tests use these counts to exercise token thresholds.
     * `usage` is required unless `error` is set; errors omit usage.
     */
    usage?: MockUsage;
    delayMs?: number;
    /** `model` overrides the response model; otherwise the response echoes the request model. */
    model?: string;
    /** The stream closes after this many SSE frames, before `message_stop`, as a provider disconnect does. */
    abortAfterFrames?: number;
    /**
     * `error` returns an error response instead of an assistant message.
     * `error` simulates provider failures such as context overflow, rate limits, and authentication errors.
     * The harness emits Anthropic-shaped error bodies for rate limits and authentication errors.
     * The harness emits an Anthropic-shaped error body with the supplied HTTP status, type, and message.
     * `parseAPICallError` and Eidnara's overflow detector match these error bodies.
     */
    error?: {
        /** `status` is the HTTP status code, such as 400 for overflow or 413 for an oversized payload. */
        status: number;
        /** `type` is the Anthropic `error.type` value, such as `invalid_request_error`. */
        type: string;
        /** `message` is regex-matched to detect context overflow. */
        message: string;
    };
}

export interface CapturedRequest {
    receivedAt: number;
    /** Set when the mock has finished producing the response for this request. */
    responseCompletedAt?: number;
    method: string;
    path: string;
    headers: Record<string, string>;
    body: {
        model?: string;
        messages?: Array<{ role: string; content: unknown }>;
        system?: unknown;
        tools?: unknown;
        [k: string]: unknown;
    };
}

export interface MockServerOptions {
    port?: number;
}

/** One cassette bound to one world variant for the mock's lifetime in that mode. */
export interface CassetteSession {
    oracle: Pick<CassetteOracle, "lookup" | "record">;
    mode: CassetteMode;
    namespace: string;
}

/**
 * Return `null` to skip to the next matcher or the default response.
 *
 * Matchers run in insertion order; the first match wins.
 * If every matcher returns `null`, the provider consults the main queue, then `defaultResponse`.
 *
 */
export type RequestMatcher = (
    body: Record<string, unknown>,
    headers: Record<string, string>,
) => MockResponse | null;

const JSON_HEADERS = { "content-type": "application/json" };
/** Well above any provider body OpenCode sends (a 200k-token context is about 1 MiB). */
const MAX_REQUEST_BODY_BYTES = 16 * 1024 * 1024;

function errorBody(type: string, extra: Record<string, unknown>): string {
    return JSON.stringify({ type: "error", error: { type, ...extra } });
}

export class MockProvider {
    private server: ReturnType<typeof Bun.serve> | null = null;
    private responses: MockResponse[] = [];
    private captured: CapturedRequest[] = [];
    private defaultResponse: MockResponse | null = null;
    private matchers: RequestMatcher[] = [];
    private defaultHitCount = 0;
    private cassette: CassetteSession | null = null;
    private cassetteMisses: CassetteMiss[] = [];
    private cassetteRefusals: CassetteRefused[] = [];
    private scriptedSelections = 0;

    async start(options: MockServerOptions = {}): Promise<{ port: number; baseURL: string }> {
        const port = options.port ?? 0; // 0 = pick any available port
        this.server = Bun.serve({
            port,
            // Bun defaults to `0.0.0.0`; bind `127.0.0.1` to restrict the scripted provider to the local host.
            hostname: "127.0.0.1",
            maxRequestBodySize: MAX_REQUEST_BODY_BYTES,
            fetch: async (req) => this.handle(req),
            // Bun's default error page carries the message, stack, and cwd; a fixed body carries nothing.
            error: () =>
                new Response(errorBody("mock_failure", {}), { status: 500, headers: JSON_HEADERS }),
        });
        const actualPort = this.server.port ?? 0;
        if (!actualPort) throw new Error("mock server failed to bind a port");
        return { port: actualPort, baseURL: `http://127.0.0.1:${actualPort}` };
    }

    async stop(): Promise<void> {
        if (this.server) {
            this.server.stop(true);
            this.server = null;
        }
    }
    script(responses: MockResponse[]): void {
        this.responses = [...responses];
    }

    /** Set a default response to return when the queue is empty. */
    setDefault(response: MockResponse): void {
        this.defaultResponse = response;
    }
    enqueue(response: MockResponse): void {
        this.responses.push(response);
    }

    /**
     * Matchers run in insertion order; the first non-`null` result determines the response.
     * If no matcher returns a response, the provider consults the main queue, then `defaultResponse`.
     */
    addMatcher(matcher: RequestMatcher): void {
        this.matchers.push(matcher);
    }
    requests(): CapturedRequest[] {
        return [...this.captured];
    }
    lastRequest(): CapturedRequest | null {
        return this.captured[this.captured.length - 1] ?? null;
    }
    /** Clears the script, the captures, the counters, and the cassette binding with its logs. */
    reset(): void {
        this.responses = [];
        this.captured = [];
        this.defaultResponse = null;
        this.matchers = [];
        this.defaultHitCount = 0;
        this.scriptedSelections = 0;
        this.cassette = null;
        this.cassetteMisses = [];
        this.cassetteRefusals = [];
    }

    /** Counts `/messages` requests that fell through matchers and the queue to `defaultResponse`. */
    defaultHits(): number {
        return this.defaultHitCount;
    }

    /** Binds the cassette every later `/messages` request goes through until `reset()`. */
    useCassette(session: CassetteSession): void {
        this.cassette = session;
    }

    /** Every typed miss the replay produced; strict replay makes this empty or one entry. */
    cassetteMissLog(): CassetteMiss[] {
        return [...this.cassetteMisses];
    }

    /** Every oracle refusal, recording or replay; a refused exchange was never persisted. */
    cassetteRefusalLog(): CassetteRefused[] {
        return [...this.cassetteRefusals];
    }

    /** How many requests entered the scripted-selection block; a whole replay run leaves it at zero. */
    scriptedSelectionCount(): number {
        return this.scriptedSelections;
    }

    private async handle(req: Request): Promise<Response> {
        let url: URL;
        try {
            url = new URL(req.url);
        } catch {
            return new Response(
                JSON.stringify({
                    error: "bad_request",
                    message: "invalid request URL",
                }),
                {
                    status: 400,
                    headers: JSON_HEADERS,
                },
            );
        }
        const method = req.method;

        // `baseURL` may include `/v1`, so the mock accepts both paths.
        const isMessages = url.pathname === "/messages" || url.pathname === "/v1/messages";

        if (method === "POST" && isMessages) {
            const bodyText = await req.text();
            let body: Record<string, unknown> = {};
            try {
                body = JSON.parse(bodyText) as Record<string, unknown>;
            } catch {
                body = {};
            }

            const headers: Record<string, string> = {};
            req.headers.forEach((value, key) => {
                headers[key] = value;
            });

            const captured: CapturedRequest = {
                receivedAt: Date.now(),
                method,
                path: url.pathname,
                headers,
                body,
            };
            this.captured.push(captured);

            const oracleRequest: OracleRequest = {
                path: url.pathname,
                headers,
                body_text: bodyText,
            };
            if (this.cassette?.mode === "replay") {
                return this.replay(this.cassette, oracleRequest, captured);
            }

            this.scriptedSelections += 1;
            // First non-null matcher result determines the response.
            let matcherResponse: MockResponse | null = null;
            for (const matcher of this.matchers) {
                const resp = matcher(body, headers);
                if (resp !== null) {
                    matcherResponse = resp;
                    break;
                }
            }
            const fromQueue = matcherResponse === null ? this.responses.shift() : undefined;
            const scripted = matcherResponse ?? fromQueue ?? this.defaultResponse;
            if (
                matcherResponse === null &&
                fromQueue === undefined &&
                this.defaultResponse !== null
            ) {
                this.defaultHitCount += 1;
            }
            if (!scripted) {
                return new Response(
                    errorBody("mock_error", { message: "No scripted response available" }),
                    { status: 500, headers: JSON_HEADERS },
                );
            }

            if (scripted.delayMs && scripted.delayMs > 0) {
                await Bun.sleep(scripted.delayMs);
            }
            captured.responseCompletedAt = Date.now();

            const produced = produce(scripted, body);
            if (this.cassette?.mode === "record") {
                try {
                    await this.cassette.oracle.record(
                        this.cassette.namespace,
                        oracleRequest,
                        produced,
                    );
                } catch (error) {
                    return this.refuse("redaction_refused", error);
                }
            }
            return serve(produced);
        }

        return new Response(JSON.stringify({ error: "not_found", path: url.pathname }), {
            status: 404,
            headers: JSON_HEADERS,
        });
    }

    private async replay(
        session: CassetteSession,
        request: OracleRequest,
        captured: CapturedRequest,
    ): Promise<Response> {
        let outcome: Awaited<ReturnType<CassetteOracle["lookup"]>>;
        try {
            outcome = await session.oracle.lookup(session.namespace, request);
        } catch (error) {
            return this.refuse("cassette_refused", error);
        }
        captured.responseCompletedAt = Date.now();
        if ("miss" in outcome) {
            this.cassetteMisses.push(outcome.miss);
            return new Response(errorBody("cassette_miss", { ...outcome.miss }), {
                status: 400,
                headers: JSON_HEADERS,
            });
        }
        return serve(outcome.hit.response);
    }

    /**
     * Every oracle failure becomes a typed 400 naming only the refusal kind: a `CassetteRefused`
     * keeps its Rust kind, anything else (a dead child, an unreadable reply) is `OracleUnavailable`.
     * No message text is served, because an unexpected error may quote what it was handling.
     */
    private refuse(type: string, error: unknown): Response {
        const refused =
            error instanceof CassetteRefused ? error : new CassetteRefused("OracleUnavailable", "");
        this.cassetteRefusals.push(refused);
        return new Response(errorBody(type, { kind: refused.kind }), {
            status: 400,
            headers: JSON_HEADERS,
        });
    }
}

/** Builds the exact response the script describes, as status, content type, and frames. */
function produce(scripted: MockResponse, body: Record<string, unknown>): RecordedResponse {
    const single = (status: number, text: string): RecordedResponse => ({
        status,
        content_type: "application/json",
        frames: [text],
        aborted: false,
    });
    // When `scripted.error` is set, emit an Anthropic-shaped JSON error body with the requested HTTP status.
    // The error response bypasses the SSE streaming path.
    if (scripted.error) {
        return single(
            scripted.error.status,
            errorBody(scripted.error.type, { message: scripted.error.message }),
        );
    }
    const usage = scripted.usage;
    if (!usage) {
        return single(
            500,
            errorBody("mock_error", { message: "MockResponse requires `usage` or `error`" }),
        );
    }
    const content = scripted.content ?? [{ type: "text", text: scripted.text ?? "OK" }];
    const respModel =
        scripted.model ?? (typeof body.model === "string" ? body.model : "mock-model");
    const messageId = `msg_${crypto.randomUUID().replace(/-/g, "").slice(0, 24)}`;
    const fullUsage = {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens ?? 0,
        cache_read_input_tokens: usage.cache_read_input_tokens ?? 0,
    };
    const stopReason = scripted.stop_reason ?? "end_turn";
    if (body.stream !== true) {
        return single(
            200,
            JSON.stringify({
                id: messageId,
                type: "message",
                role: "assistant",
                model: respModel,
                content,
                stop_reason: stopReason,
                stop_sequence: null,
                usage: fullUsage,
            }),
        );
    }
    const frames = sseFrames(content, respModel, messageId, fullUsage, stopReason);
    const cut = scripted.abortAfterFrames;
    if (cut !== undefined && cut < frames.length) {
        return {
            status: 200,
            content_type: "text/event-stream",
            frames: frames.slice(0, cut),
            aborted: true,
        };
    }
    return { status: 200, content_type: "text/event-stream", frames, aborted: false };
}

/**
 * The stream emits `message_start`, then per block `content_block_start`, `content_block_delta`,
 * `content_block_stop`, then `message_delta` with the final usage, then `message_stop`.
 */
function sseFrames(
    content: unknown[],
    model: string,
    messageId: string,
    usage: Record<string, number>,
    stopReason: string,
): string[] {
    const frames: string[] = [];
    const send = (event: string, data: Record<string, unknown>) => {
        frames.push(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
    };
    send("message_start", {
        type: "message_start",
        message: {
            id: messageId,
            type: "message",
            role: "assistant",
            model,
            content: [],
            stop_reason: null,
            stop_sequence: null,
            usage: {
                input_tokens: usage.input_tokens,
                output_tokens: 0,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            },
        },
    });
    content.forEach((block: unknown, index: number) => {
        const blk = block as {
            type?: string;
            text?: string;
            thinking?: string;
            signature?: string;
            data?: string;
        };
        const blockType = blk.type ?? "text";

        if (blockType === "text") {
            send("content_block_start", {
                type: "content_block_start",
                index,
                content_block: { type: "text", text: "" },
            });
            send("content_block_delta", {
                type: "content_block_delta",
                index,
                delta: {
                    type: "text_delta",
                    text: blk.text ?? "",
                },
            });
            send("content_block_stop", {
                type: "content_block_stop",
                index,
            });
        } else if (blockType === "thinking") {
            // A thinking block emits a start event, a thinking delta, and a signature delta.
            // 4. content_block_stop
            send("content_block_start", {
                type: "content_block_start",
                index,
                content_block: {
                    type: "thinking",
                    thinking: "",
                },
            });
            if (blk.thinking) {
                send("content_block_delta", {
                    type: "content_block_delta",
                    index,
                    delta: {
                        type: "thinking_delta",
                        thinking: blk.thinking,
                    },
                });
            }
            if (blk.signature) {
                send("content_block_delta", {
                    type: "content_block_delta",
                    index,
                    delta: {
                        type: "signature_delta",
                        signature: blk.signature,
                    },
                });
            }
            send("content_block_stop", {
                type: "content_block_stop",
                index,
            });
        } else if (blockType === "redacted_thinking") {
            // The redacted-thinking block emits no deltas; its start event carries opaque `data`.
            send("content_block_start", {
                type: "content_block_start",
                index,
                content_block: {
                    type: "redacted_thinking",
                    data: blk.data ?? "",
                },
            });
            send("content_block_stop", {
                type: "content_block_stop",
                index,
            });
        } else if (blockType === "tool_use") {
            const toolBlock = block as {
                type: "tool_use";
                id?: string;
                name?: string;
                input?: Record<string, unknown>;
            };
            send("content_block_start", {
                type: "content_block_start",
                index,
                content_block: {
                    type: "tool_use",
                    id: toolBlock.id ?? `toolu_${index}`,
                    name: toolBlock.name ?? "mock_tool",
                    input: {},
                },
            });
            send("content_block_delta", {
                type: "content_block_delta",
                index,
                delta: {
                    type: "input_json_delta",
                    partial_json: JSON.stringify(toolBlock.input ?? {}),
                },
            });
            send("content_block_stop", {
                type: "content_block_stop",
                index,
            });
        } else {
            // Non-text blocks pass through unchanged.
            send("content_block_start", {
                type: "content_block_start",
                index,
                content_block: block,
            });
            send("content_block_stop", {
                type: "content_block_stop",
                index,
            });
        }
    });

    send("message_delta", {
        type: "message_delta",
        delta: {
            stop_reason: stopReason,
            stop_sequence: null,
        },
        usage: {
            // `message_delta.usage` repeats the cumulative usage sent in `message_start`.
            input_tokens: usage.input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            output_tokens: usage.output_tokens,
        },
    });
    send("message_stop", { type: "message_stop" });
    return frames;
}

/** Serves a produced or recorded response byte for byte. */
function serve(response: RecordedResponse): Response {
    if (response.content_type !== "text/event-stream") {
        return new Response(response.frames.join(""), {
            status: response.status,
            headers: { "content-type": response.content_type },
        });
    }
    const encoder = new TextEncoder();
    // An aborted recording simply has no `message_stop` frame; the stream ends where the
    // recording ended, which is what a client of a disconnected provider observes.
    const stream = new ReadableStream({
        start(controller) {
            for (const frame of response.frames) controller.enqueue(encoder.encode(frame));
            controller.close();
        },
    });
    return new Response(stream, {
        status: response.status,
        headers: {
            "content-type": "text/event-stream",
            "cache-control": "no-cache",
            connection: "keep-alive",
        },
    });
}
