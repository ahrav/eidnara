/**
 * Record-and-forward mode for the Messages mock: each request OpenCode sends is forwarded,
 * byte for byte, to one explicitly selected HTTPS Messages endpoint, and the provider's response
 * goes back to OpenCode. Every send is admitted against limits frozen at construction: a call
 * count, the largest `max_tokens` a request may carry, a per-send timeout, and a total spend cap
 * that every send, retry, and fallback counts against. A failed, timed-out, or refused send stops
 * the run; later requests are refused without a send.
 *
 * Credentials come from a callback at send time and reach only the outbound request headers.
 * Captures hold the exact forwarded bytes, redacted headers, and the bounded response bytes, and
 * publish privately outside the repository.
 */

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, realpathSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { publishJsonAtomically } from "../atomic-publish";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "../compression-fidelity/corpus";

export interface ForwardLimits {
    /** Sends the run may attempt, retries and fallbacks included. */
    maxCalls: number;
    /** The largest `max_tokens` a forwarded request may carry. */
    maxOutputTokens: number;
    /** Deadline for one send, response body included. */
    timeoutMs: number;
    /** USD every send's cost, known or bounded, counts against. */
    spendCapUsd: number;
}

export interface ForwardConfig {
    /** The HTTPS Messages endpoint, such as `https://api.anthropic.com/v1/messages`. */
    upstreamURL: string;
    /**
     * The model id as the provider echoes it in responses. A response naming another model, or
     * none, stops the run, so an alias the provider resolves to a dated id stops at the first
     * response.
     */
    model: string;
    /** The model's context window, which OpenCode is configured with. */
    contextLimit: number;
    /**
     * The SHA-256 of the reviewed synthetic corpus the run serves; any other value is refused,
     * so forwarding carries only reviewed corpus input.
     */
    corpusSha256: string;
    /**
     * USD per million tokens. `inputPerMTok` must be the highest input-side price the model
     * charges, cache writes and any `anthropic-beta` pricing the client enables included, so a
     * bound built from it never undercounts. A response whose stated usage costs more than its
     * reservation stops the run.
     */
    pricing: { inputPerMTok: number; outputPerMTok: number };
    limits: ForwardLimits;
    /** Outbound credential headers, read once per send and never stored. */
    credentials: () => Record<string, string>;
    /** Outbound transport; the global `fetch` when absent. */
    fetch?: (url: string, init: RequestInit) => Promise<Response>;
}

/** Response bytes kept per exchange; a longer response is served whole and captured truncated. */
export const MAX_CAPTURED_RESPONSE_BYTES = 4 * 1024 * 1024;
/** Response bytes read per send; a longer response is cut off and the send fails. */
export const MAX_RESPONSE_BYTES = 4 * MAX_CAPTURED_RESPONSE_BYTES;
/** Refusal reasons kept in a report; `refused` counts every refusal, retained or not. */
export const MAX_RETAINED_REFUSALS = 32;

const REDACTED_HEADERS = new Set([
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "anthropic-api-key",
    "cookie",
    "set-cookie",
    "x-amz-security-token",
]);
/** Request headers the provider needs; everything else, the client's own key included, stays. */
const FORWARDED_HEADERS = ["content-type", "accept", "anthropic-version", "anthropic-beta"];

export type SendOutcome = "acknowledged" | "provider_error" | "ambiguous";

export interface ForwardedExchange {
    index: number;
    /** The `tool_result` ids the request answers. */
    tool_results: string[];
    /** The `tool_use` ids the response asks for. */
    tool_uses: string[];
    request: {
        headers: Record<string, string>;
        /** SHA-256 of the bytes the mock received, which are the bytes it sent. */
        body_sha256: string;
        body_bytes: number;
        body_text: string;
        reserved_usd: number;
    };
    response: {
        outcome: SendOutcome;
        status: number | null;
        headers: Record<string, string>;
        body_sha256: string | null;
        body_bytes: number;
        body_text: string;
        truncated: boolean;
        stop_reason: string | null;
        /** The model the response names; `null` when it names none. */
        model: string | null;
        usage: Usage | null;
        /** The cost charged against the cap: from usage when known, else the reservation. */
        cost_usd: number;
        cost_known: boolean;
    } | null;
}

export interface Usage {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens: number;
    cache_read_input_tokens: number;
}

export interface ForwardingReport {
    /** Distinguishes forwarded execution from scripted responses, whatever model a body names. */
    mode: "forward";
    upstream_url: string;
    corpus_sha256: string;
    model: string;
    context_limit: number;
    limits: ForwardLimits;
    pricing: ForwardConfig["pricing"];
    attempted_sends: number;
    acknowledged_responses: number;
    spent_usd: number;
    stopped: string | null;
    /** Every refused request, including those past `MAX_RETAINED_REFUSALS`. */
    refused: number;
    refusals: string[];
    complete: boolean;
    incomplete_reasons: string[];
    exchanges: ForwardedExchange[];
}

function positive(value: unknown): value is number {
    return typeof value === "number" && Number.isFinite(value) && value > 0;
}

/** Rejects a config that lacks an HTTPS Messages endpoint, a model, prices, or any limit. */
export function validateForwardConfig(config: ForwardConfig): Readonly<ForwardConfig> {
    let url: URL;
    try {
        url = new URL(config.upstreamURL);
    } catch {
        throw new Error("forwarding needs an explicit upstream URL");
    }
    if (url.protocol !== "https:") throw new Error("forwarding accepts only an HTTPS provider");
    if (!url.pathname.endsWith("/messages")) {
        throw new Error("forwarding accepts only a Messages endpoint");
    }
    if (url.username || url.password || url.search || url.hash) {
        throw new Error("the upstream URL carries no credential, query, or fragment");
    }
    if (typeof config.model !== "string" || config.model === "") {
        throw new Error("forwarding needs an explicit model");
    }
    if (!positive(config.contextLimit) || !Number.isInteger(config.contextLimit)) {
        throw new Error("forwarding needs the model's whole-number context limit");
    }
    if (config.corpusSha256 !== COMPRESSION_FIDELITY_CORPUS_SHA256) {
        throw new Error("forwarding serves only the reviewed synthetic corpus");
    }
    const limits = config.limits ?? ({} as ForwardLimits);
    for (const name of ["maxCalls", "maxOutputTokens", "timeoutMs", "spendCapUsd"] as const) {
        if (!positive(limits[name])) throw new Error(`forwarding needs a positive limits.${name}`);
    }
    if (!Number.isInteger(limits.maxCalls) || !Number.isInteger(limits.maxOutputTokens)) {
        throw new Error("limits.maxCalls and limits.maxOutputTokens are whole numbers");
    }
    if (!positive(config.pricing?.inputPerMTok) || !positive(config.pricing?.outputPerMTok)) {
        throw new Error("forwarding needs positive input and output prices");
    }
    if (typeof config.credentials !== "function") {
        throw new Error("forwarding needs a credential callback");
    }
    return Object.freeze({
        ...config,
        limits: Object.freeze({ ...limits }),
        pricing: Object.freeze({ ...config.pricing }),
    });
}

function sha256(bytes: Uint8Array): string {
    return createHash("sha256").update(bytes).digest("hex");
}

/** `headers` with credential-bearing values replaced. */
export function redact(headers: Record<string, string>): Record<string, string> {
    const kept: Record<string, string> = {};
    for (const [name, value] of Object.entries(headers)) {
        kept[name.toLowerCase()] = REDACTED_HEADERS.has(name.toLowerCase()) ? "[redacted]" : value;
    }
    return kept;
}

function headerRecord(headers: Headers): Record<string, string> {
    const record: Record<string, string> = {};
    headers.forEach((value, key) => {
        record[key] = value;
    });
    return record;
}

function isCount(value: unknown): value is number {
    return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

export interface ResponseFacts {
    usage: Usage | null;
    stopReason: string | null;
    /** The `model` the JSON body or an SSE event's `message` names; `null` when none does. */
    model: string | null;
    /** The `tool_use` ids the response asks for. */
    toolUses: string[];
    /** Why an SSE stream is unfinished: an `error` event or no `message_stop`; `null` when finished. */
    unfinished: string | null;
    /**
     * Why the body is not a Messages message: no JSON `type: "message"` with a `content` array,
     * or no SSE `message_start` carrying a `message`; `null` when it is one.
     */
    malformed: string | null;
}

function isObject(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === "object";
}

/**
 * `readResponse` reads the keys `usage`, `stop_reason`, and `content_block` and the event types
 * `message_stop` and `error`. JSON spells each as the literal quoted string or with a `\u`
 * escape, so a `data:` line that names any of them contains one of these six patterns.
 */
const FACT_KEY = /"(?:usage|stop_reason|content_block|message_stop|error)"|\\u/g;

function factLines(text: string): string[] {
    const starts: number[] = [];
    FACT_KEY.lastIndex = 0;
    for (let match = FACT_KEY.exec(text); match; match = FACT_KEY.exec(text)) {
        starts.push(text.lastIndexOf("\n", match.index) + 1);
        const end = text.indexOf("\n", match.index);
        if (end < 0) break;
        FACT_KEY.lastIndex = end + 1;
    }
    return starts.map((start) => {
        const end = text.indexOf("\n", start);
        return text.slice(start, end < 0 ? text.length : end);
    });
}

/**
 * The usage, stop reason, model, and `tool_use` ids of a JSON or SSE Messages response, read in
 * one pass over its events. Usage is `null` unless the response states input and output counts;
 * absent cache counts are zero. SSE usage needs both a `message_delta` carrying usage and a
 * `message_stop` event, because `message_start` carries provisional counts; any `error` event
 * makes it `null`. An SSE `data:` line that is not a JSON object is skipped.
 */
export function readResponse(contentType: string, text: string): ResponseFacts {
    const fields: Record<string, unknown> = {};
    let stopReason: string | null = null;
    let model: string | null = null;
    let unfinished: string | null = null;
    let message = false;
    let finalUsage = true;
    const blocks: Array<Record<string, unknown>> = [];
    const take = (usage: unknown) => {
        if (isObject(usage)) Object.assign(fields, usage);
    };
    const name = (value: unknown) => {
        if (typeof value === "string") model = value;
    };
    if (contentType.toLowerCase().includes("text/event-stream")) {
        let stopped = false;
        let errored = false;
        finalUsage = false;
        for (const line of factLines(text)) {
            if (!line.startsWith("data:")) continue;
            let event: unknown;
            try {
                event = JSON.parse(line.slice(5));
            } catch {
                continue;
            }
            if (!isObject(event)) continue;
            if (isObject(event.message)) {
                take(event.message.usage);
                name(event.message.model);
                if (event.type === "message_start") message = true;
            }
            take(event.usage);
            if (event.type === "message_delta" && isObject(event.usage)) finalUsage = true;
            if (event.type === "message_stop") stopped = true;
            if (event.type === "error") errored = true;
            if (isObject(event.delta) && typeof event.delta.stop_reason === "string") {
                stopReason = event.delta.stop_reason;
            }
            if (isObject(event.content_block)) blocks.push(event.content_block);
        }
        if (errored) unfinished = "stream carried an error event";
        else if (!stopped) unfinished = "stream ended before message_stop";
    } else {
        let body: unknown;
        try {
            body = JSON.parse(text);
        } catch {
            body = undefined;
        }
        if (isObject(body)) {
            take(body.usage);
            name(body.model);
            if (typeof body.stop_reason === "string") stopReason = body.stop_reason;
            blocks.push(...blocksOf(body.content));
            message = body.type === "message" && Array.isArray(body.content);
        }
    }
    const malformed = message ? null : "is not a Messages message";
    const toolUses = blocks
        .filter((block) => block.type === "tool_use" && typeof block.id === "string")
        .map((block) => block.id as string);
    const cacheWrite = fields.cache_creation_input_tokens ?? 0;
    const cacheRead = fields.cache_read_input_tokens ?? 0;
    if (
        unfinished !== null ||
        !finalUsage ||
        !isCount(fields.input_tokens) ||
        !isCount(fields.output_tokens) ||
        !isCount(cacheWrite) ||
        !isCount(cacheRead)
    ) {
        return { usage: null, stopReason, model, toolUses, unfinished, malformed };
    }
    return {
        usage: {
            input_tokens: fields.input_tokens,
            output_tokens: fields.output_tokens,
            cache_creation_input_tokens: cacheWrite,
            cache_read_input_tokens: cacheRead,
        },
        stopReason,
        model,
        toolUses,
        unfinished,
        malformed,
    };
}

/** The body's bytes, or a `ResponseTooLarge` error once more than `cap` bytes have arrived. */
async function readBounded(response: Response, cap: number): Promise<Uint8Array<ArrayBuffer>> {
    const reader = response.body?.getReader();
    if (!reader) return new Uint8Array(0);
    const chunks: Uint8Array[] = [];
    let total = 0;
    for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.byteLength;
        if (total > cap) {
            await reader.cancel();
            const error = new Error(`response above ${cap} bytes`);
            error.name = "ResponseTooLarge";
            throw error;
        }
        chunks.push(value);
    }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
        bytes.set(chunk, offset);
        offset += chunk.byteLength;
    }
    return bytes;
}

function errorResponse(type: string, message: string): Response {
    return new Response(JSON.stringify({ type: "error", error: { type, message } }), {
        status: 400,
        headers: { "content-type": "application/json" },
    });
}

function blocksOf(value: unknown): Array<Record<string, unknown>> {
    return Array.isArray(value) ? value.filter(isObject) : [];
}

/** The `tool_result` ids a parsed Messages request answers. */
function toolResultIds(request: Record<string, unknown>): string[] {
    return blocksOf(request.messages).flatMap((message) =>
        blocksOf(message.content)
            .filter(
                (block) => block.type === "tool_result" && typeof block.tool_use_id === "string",
            )
            .map((block) => block.tool_use_id as string),
    );
}

function unanswered(reserved: number): NonNullable<ForwardedExchange["response"]> {
    return {
        outcome: "ambiguous",
        status: null,
        headers: {},
        body_sha256: null,
        body_bytes: 0,
        body_text: "",
        truncated: false,
        stop_reason: null,
        model: null,
        usage: null,
        cost_usd: reserved,
        cost_known: false,
    };
}

function copyExchange(exchange: ForwardedExchange): ForwardedExchange {
    const { request, response } = exchange;
    return {
        ...exchange,
        tool_results: [...exchange.tool_results],
        tool_uses: [...exchange.tool_uses],
        request: { ...request, headers: { ...request.headers } },
        response: response && {
            ...response,
            headers: { ...response.headers },
            usage: response.usage && { ...response.usage },
        },
    };
}

export class Forwarder {
    private readonly config: Readonly<ForwardConfig>;
    private readonly exchanges: ForwardedExchange[] = [];
    private readonly refusals: string[] = [];
    private refused = 0;
    /** Aborts every send still in flight when the mock stops. */
    private readonly lifecycle = new AbortController();
    private readonly unhashed = new Map<ForwardedExchange, Uint8Array>();
    private spent = 0;
    private stopped: string | null = null;

    constructor(config: ForwardConfig) {
        this.config = validateForwardConfig(config);
    }

    private refuse(reason: string): Response {
        this.refused += 1;
        if (this.refusals.length < MAX_RETAINED_REFUSALS) this.refusals.push(reason);
        this.stopped ??= reason;
        return errorResponse("forwarding_refused", reason);
    }

    private price(inputTokens: number, outputTokens: number): number {
        const { inputPerMTok, outputPerMTok } = this.config.pricing;
        return (inputTokens * inputPerMTok + outputTokens * outputPerMTok) / 1_000_000;
    }

    /** The send's reservation, or the reason the limits refuse it. */
    private admit(
        json: unknown,
        bytes: number,
    ): { reserved: number; toolResults: string[] } | { refused: string } {
        if (this.stopped) return { refused: `stopped: ${this.stopped}` };
        const { limits, model } = this.config;
        if (json === undefined) return { refused: "unreadable request body" };
        if (!isObject(json) || Array.isArray(json)) {
            return { refused: "request body is not a JSON object" };
        }
        const parsed = json;
        if (parsed.model !== model)
            return { refused: `request names model ${String(parsed.model)}` };
        if (!isCount(parsed.max_tokens) || parsed.max_tokens > limits.maxOutputTokens) {
            return { refused: "max_tokens is absent or above limits.maxOutputTokens" };
        }
        if (this.exchanges.length >= limits.maxCalls) return { refused: "limits.maxCalls reached" };
        // Body bytes bound the body's own input tokens; input the provider adds beyond the body
        // is caught after the send, when usage above the reservation stops the run.
        const reserved = this.price(bytes, parsed.max_tokens);
        if (this.spent + reserved > limits.spendCapUsd) {
            return { refused: "limits.spendCapUsd would be exceeded" };
        }
        return { reserved, toolResults: toolResultIds(parsed) };
    }

    /** Ends every send still in flight; the mock calls this when it stops. */
    close(): void {
        this.lifecycle.abort(new DOMException("the forwarding mock stopped", "AbortError"));
    }

    /**
     * Sends `body` and reads the response, up to `MAX_RESPONSE_BYTES`, before the deadline or
     * the mock's stop, whichever comes first.
     */
    private async timedSend(
        body: Uint8Array<ArrayBuffer>,
        headers: Record<string, string>,
    ): Promise<readonly [Response, Uint8Array<ArrayBuffer>]> {
        const deadline = AbortSignal.any([
            AbortSignal.timeout(this.config.limits.timeoutMs),
            this.lifecycle.signal,
        ]);
        const expired = new Promise<never>((_, reject) => {
            deadline.addEventListener("abort", () => reject(deadline.reason), { once: true });
        });
        // A send that settles first leaves the deadline to fire unobserved.
        expired.catch(() => {});
        const send = this.config.fetch ?? fetch;
        return Promise.race([
            (async () => {
                const sent = await send(this.config.upstreamURL, {
                    method: "POST",
                    headers,
                    body,
                    signal: deadline,
                    // A redirect would move the send to an unselected target.
                    redirect: "error",
                });
                return [sent, await readBounded(sent, MAX_RESPONSE_BYTES)] as const;
            })(),
            expired,
        ]);
    }

    /**
     * Forwards `body` and returns the provider's response, or a non-retryable refusal when a
     * limit, the model, or an earlier stop forbids the send. `text` is `body` decoded as UTF-8
     * and `json` is `JSON.parse(text)`, or `undefined` when `text` is not JSON.
     */
    async forward(
        body: Uint8Array<ArrayBuffer>,
        text: string,
        json: unknown,
        headers: Record<string, string>,
    ): Promise<Response> {
        const admitted = this.admit(json, body.byteLength);
        if ("refused" in admitted) return this.refuse(admitted.refused);
        const { reserved, toolResults } = admitted;
        const outbound: Record<string, string> = {};
        for (const name of FORWARDED_HEADERS) {
            if (headers[name] !== undefined) outbound[name] = headers[name];
        }
        let credentials: Record<string, string>;
        try {
            credentials = this.config.credentials();
        } catch {
            return this.refuse("the credential callback failed");
        }
        const exchange: ForwardedExchange = {
            index: this.exchanges.length,
            tool_results: toolResults,
            tool_uses: [],
            request: {
                headers: redact(headers),
                body_sha256: "",
                body_bytes: body.byteLength,
                body_text: text,
                reserved_usd: reserved,
            },
            response: null,
        };
        this.exchanges.push(exchange);
        // The send proceeds while the digest is pending.
        this.unhashed.set(exchange, body);
        crypto.subtle.digest("SHA-256", body).then(
            (digest) => {
                if (this.unhashed.delete(exchange)) {
                    exchange.request.body_sha256 = Buffer.from(digest).toString("hex");
                }
            },
            () => {},
        );
        // The reservation counts until the response settles the real cost.
        this.spent += reserved;
        let response: Response;
        let bytes: Uint8Array<ArrayBuffer>;
        try {
            [response, bytes] = await this.timedSend(body, { ...outbound, ...credentials });
        } catch (error) {
            const name = error instanceof Error ? error.name : "Error";
            exchange.response = unanswered(reserved);
            this.stopped ??= `send ${exchange.index} ended without a response (${name})`;
            return errorResponse("forwarding_failed", this.stopped);
        }
        const contentType = response.headers.get("content-type") ?? "";
        const responseText = new TextDecoder().decode(bytes);
        const { usage, stopReason, model, toolUses, unfinished, malformed } = readResponse(
            contentType,
            responseText,
        );
        exchange.tool_uses = toolUses;
        const cost = usage
            ? this.price(
                  usage.input_tokens +
                      usage.cache_creation_input_tokens +
                      usage.cache_read_input_tokens,
                  usage.output_tokens,
              )
            : reserved;
        this.spent += cost - reserved;
        const captured = bytes.subarray(0, MAX_CAPTURED_RESPONSE_BYTES);
        const truncated = captured.byteLength < bytes.byteLength;
        exchange.response = {
            outcome: response.ok ? "acknowledged" : "provider_error",
            status: response.status,
            headers: redact(headerRecord(response.headers)),
            body_sha256: sha256(bytes),
            body_bytes: bytes.byteLength,
            body_text: truncated ? new TextDecoder().decode(captured) : responseText,
            truncated,
            stop_reason: stopReason,
            model,
            usage,
            cost_usd: cost,
            cost_known: usage !== null,
        };
        if (!response.ok)
            this.stopped ??= `send ${exchange.index} returned HTTP ${response.status}`;
        else if (unfinished) this.stopped ??= `send ${exchange.index} ${unfinished}`;
        else if (malformed) this.stopped ??= `send ${exchange.index} ${malformed}`;
        else if (model === null) this.stopped ??= `send ${exchange.index} states no model`;
        else if (model !== this.config.model) {
            this.stopped ??= `send ${exchange.index} names model ${model}`;
        }
        if (cost > reserved) this.stopped ??= `send ${exchange.index} cost above its reservation`;
        return new Response(bytes, {
            status: response.status,
            headers: contentType ? { "content-type": contentType } : {},
        });
    }

    /** The run so far: the frozen limits, every send and response, and whether it is complete. */
    report(): ForwardingReport {
        for (const [exchange, body] of this.unhashed) {
            exchange.request.body_sha256 = sha256(body);
        }
        this.unhashed.clear();
        const settled = this.exchanges.flatMap((e) => (e.response ? [e.response] : []));
        const acknowledged = settled.filter((r) => r.outcome === "acknowledged");
        const reasons: string[] = [];
        if (this.stopped) reasons.push(`stopped: ${this.stopped}`);
        if (this.exchanges.length === 0) reasons.push("no send");
        if (settled.length < this.exchanges.length) reasons.push("a send is in flight");
        if (settled.some((r) => r.truncated)) reasons.push("capture truncated");
        if (settled.some((r) => !r.cost_known)) reasons.push("cost unknown for a send");
        if (acknowledged.some((r) => r.stop_reason === null)) {
            reasons.push("a response states no stop reason");
        }
        if (this.spent > this.config.limits.spendCapUsd) reasons.push("spend above the cap");
        const answered = this.exchanges.every((exchange) =>
            exchange.tool_uses.every((id) =>
                this.exchanges.some(
                    (later) => later.index > exchange.index && later.tool_results.includes(id),
                ),
            ),
        );
        if (!answered) reasons.push("tool loop unfinished");
        return {
            mode: "forward",
            upstream_url: this.config.upstreamURL,
            corpus_sha256: this.config.corpusSha256,
            model: this.config.model,
            context_limit: this.config.contextLimit,
            limits: { ...this.config.limits },
            pricing: { ...this.config.pricing },
            attempted_sends: this.exchanges.length,
            acknowledged_responses: acknowledged.length,
            spent_usd: this.spent,
            stopped: this.stopped,
            refused: this.refused,
            refusals: [...this.refusals],
            complete: reasons.length === 0,
            incomplete_reasons: reasons,
            exchanges: this.exchanges.map(copyExchange),
        };
    }
}

/** The repository root: captures never land beneath it. */
const REPOSITORY_ROOT = resolve(import.meta.dir, "../../../..");

/**
 * Every existing ancestor of `real` is owned by this user or root and is either closed to
 * group and other writes or sticky, so no other local user can rename the checked directory
 * away between its check and the write. It runs before creation and again after, so a
 * component another user created in between is refused rather than trusted.
 */
function requireTrustedAncestors(real: string): void {
    const uid = process.getuid?.();
    let existing = real;
    while (!existsSync(existing)) existing = dirname(existing);
    for (let ancestor = existing; ; ancestor = dirname(ancestor)) {
        const stat = statSync(ancestor);
        const ownedByTrusted = stat.uid === uid || stat.uid === 0;
        const othersWrite = (stat.mode & 0o022) !== 0 && (stat.mode & 0o1000) === 0;
        if (!ownedByTrusted || othersWrite) {
            throw new Error(`${ancestor} is writable by others or not owned by this user or root`);
        }
        if (ancestor === dirname(ancestor)) break;
    }
}

/**
 * Publishes `report` as `<dir>/forwarding-<label>.json` with mode `0600` in an owner-only
 * directory outside the repository. The directory is created `0700` when absent; an existing
 * one must already be `0700` and owned by this user, and every ancestor must be trusted.
 */
export function publishForwardingReport(
    report: ForwardingReport,
    dir: string,
    label: string,
): string {
    if (!/^[A-Za-z0-9._-]+$/.test(label) || label.startsWith(".")) {
        throw new Error(`${label} is not a plain file label`);
    }
    if (dir.split(/[\\/]/).includes("..")) throw new Error(`${dir} names a parent directory`);
    const target = resolve(dir);
    let existing = target;
    while (!existsSync(existing)) existing = dirname(existing);
    const real = join(realpathSync(existing), relative(existing, target));
    const inside = relative(realpathSync(REPOSITORY_ROOT), real);
    if (
        inside === "" ||
        (inside !== ".." && !inside.startsWith(`..${sep}`) && !isAbsolute(inside))
    ) {
        throw new Error(`${real} is inside the repository`);
    }
    requireTrustedAncestors(real);
    mkdirSync(real, { recursive: true, mode: 0o700 });
    requireTrustedAncestors(real);
    const stat = statSync(real);
    if ((stat.mode & 0o777) !== 0o700 || stat.uid !== process.getuid?.()) {
        throw new Error(`${real} is not an owner-only directory`);
    }
    const path = join(real, `forwarding-${label}.json`);
    publishJsonAtomically(report, path, { mode: 0o600 });
    return path;
}
