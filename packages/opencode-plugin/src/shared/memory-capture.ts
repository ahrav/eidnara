import { createHash } from "node:crypto";
import type { RustModeModuleClient } from "../hooks/context/rust-mode-transform";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "./with-timeout";

export interface CaptureMessage {
    id: string;
    role: "user" | "assistant";
    text: string;
}

const CHUNK_BYTES = 16 * 1024;
const BATCH_BYTES = 64 * 1024;
const BATCH_MESSAGES = 32;
const ACK_CACHE_ENTRIES = 8192;

/** Splits at UTF-8 boundaries without dropping oversized messages. A fragment's
 * identity includes the native message id and byte offset, never model output. */
export function* captureFragments(message: CaptureMessage): Generator<CaptureMessage> {
    if (!message.text.trim()) return;
    const bytes = Buffer.from(message.text, "utf8");
    if (bytes.length <= CHUNK_BYTES && Buffer.byteLength(message.id) <= 256) {
        yield message;
        return;
    }
    const source = createHash("sha256").update(message.id).digest("hex");
    for (let start = 0; start < bytes.length; ) {
        let end = Math.min(start + CHUNK_BYTES, bytes.length);
        while (end < bytes.length && ((bytes[end] ?? 0) & 0xc0) === 0x80) end -= 1;
        const text = bytes.subarray(start, end).toString("utf8");
        if (text.trim()) yield { id: `${source}:${start}`, role: message.role, text };
        start = end;
    }
}

function record(value: unknown): Record<string, unknown> | undefined {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, unknown>)
        : undefined;
}

/** Source messages older than this are not offered for capture. */
export const CAPTURE_MAX_AGE_MS = 48 * 60 * 60 * 1000;

/** Absent flags enable capture; only an explicit `false` disables it. */
export function memoryAutoCaptureEnabled(config: {
    memory?: { enabled?: boolean; auto_promote?: boolean; auto_capture?: boolean };
}): boolean {
    const memory = config.memory;
    return (
        memory?.enabled !== false &&
        memory?.auto_promote !== false &&
        memory?.auto_capture !== false
    );
}

export interface CaptureSourceOptions {
    /** Epoch milliseconds; messages created before this are skipped. Undated messages stay. */
    notBefore?: number;
}

/** `undefined` means the age is unknown, which never excludes a message. */
function createdAtMs(value: unknown): number | undefined {
    if (typeof value === "number") return Number.isFinite(value) ? value : undefined;
    if (typeof value === "string") {
        const parsed = Date.parse(value);
        return Number.isNaN(parsed) ? undefined : parsed;
    }
    return undefined;
}

function tooOld(createdAt: number | undefined, notBefore: number | undefined): boolean {
    return createdAt !== undefined && notBefore !== undefined && createdAt < notBefore;
}

/** Native text only: no tool results, reasoning, synthetic summaries, or failed responses. */
export function* piCaptureMessages(
    entries: Iterable<unknown>,
    options: CaptureSourceOptions = {},
): Generator<CaptureMessage> {
    for (const value of entries) {
        const entry = record(value);
        const message = record(entry?.message);
        if (entry?.type !== "message" || typeof entry.id !== "string" || !message) continue;
        if (message.role !== "user" && message.role !== "assistant") continue;
        if (
            message.role === "assistant" &&
            (message.stopReason === "error" || message.stopReason === "aborted")
        )
            continue;
        if (
            tooOld(
                createdAtMs(entry.timestamp) ?? createdAtMs(message.timestamp),
                options.notBefore,
            )
        )
            continue;
        const text =
            typeof message.content === "string"
                ? message.content
                : Array.isArray(message.content)
                  ? message.content
                        .flatMap((part: unknown) => {
                            const block = record(part);
                            return block?.type === "text" && typeof block.text === "string"
                                ? [block.text]
                                : [];
                        })
                        .join("\n")
                  : "";
        if (text.trim()) yield { id: entry.id, role: message.role, text };
    }
}

/** OpenCode records creation time under `time.created`, `time_created`, or `timeCreated`. */
function openCodeCreatedAtMs(info: Record<string, unknown>): number | undefined {
    return (
        createdAtMs(record(info.time)?.created) ??
        createdAtMs(info.time_created) ??
        createdAtMs(info.timeCreated)
    );
}

/** Complete OpenCode conversation records; ignored/synthetic text is not source evidence. */
export function* openCodeCaptureMessages(
    messages: Iterable<unknown>,
    options: CaptureSourceOptions = {},
): Generator<CaptureMessage> {
    for (const value of messages) {
        const message = record(value);
        const info = record(message?.info);
        if (!info || typeof info.id !== "string" || !Array.isArray(message?.parts)) continue;
        if (info.role !== "user" && info.role !== "assistant") continue;
        if (info.summary === true || info.error !== undefined) continue;
        if (info.role === "assistant" && typeof record(info.time)?.completed !== "number") continue;
        if (tooOld(openCodeCreatedAtMs(info), options.notBefore)) continue;
        const text = message.parts
            .flatMap((part: unknown) => {
                const block = record(part);
                return block?.type === "text" &&
                    block.ignored !== true &&
                    block.synthetic !== true &&
                    typeof block.text === "string"
                    ? [block.text]
                    : [];
            })
            .join("\n");
        if (text.trim()) yield { id: info.id, role: info.role, text };
    }
}

function stateOf(response: unknown): string | undefined {
    return response !== null &&
        typeof response === "object" &&
        "state" in response &&
        typeof response.state === "string"
        ? response.state
        : undefined;
}

export interface NativeCaptureWork {
    model: string;
    system: string;
    prompt: string;
    maxOutputTokens: number;
    maxOutputBytes: number;
    maxDurationMs: number;
}

export type NativeCaptureExecutor = (
    work: NativeCaptureWork,
    signal: AbortSignal,
) => Promise<{ model: string; text: string }>;

/** Only these content-free failures cross back into daemon state or UI logs. */
export class NativeCaptureError extends Error {
    constructor(
        readonly code: "provider_unavailable" | "model_failed" | "output_limit" | "cancelled",
    ) {
        super(`Native memory capture: ${code}`);
    }
}

const LEASE_PATTERN = /^[a-f0-9]{32}$/;

/** Harness ceilings; the daemon may lower any bound but never raise one past these. */
const MAX_OUTPUT_TOKENS = 8192;
const MAX_OUTPUT_BYTES = 128 * 1024;
const MAX_DURATION_MS = 90_000;

function boundedInteger(value: unknown, ceiling: number): value is number {
    return (
        typeof value === "number" && Number.isSafeInteger(value) && value > 0 && value <= ceiling
    );
}

/** The lease of a malformed work response, when its shape alone is valid. */
function leaseOf(response: unknown): string | undefined {
    const lease = record(response)?.lease;
    return typeof lease === "string" && LEASE_PATTERN.test(lease) ? lease : undefined;
}

function nativeWork(response: unknown): { lease: string; work: NativeCaptureWork } {
    const value = record(response);
    if (
        !value ||
        typeof value.lease !== "string" ||
        !LEASE_PATTERN.test(value.lease) ||
        typeof value.model !== "string" ||
        !value.model.includes("/") ||
        value.model.length > 512 ||
        typeof value.system !== "string" ||
        Buffer.byteLength(value.system) > 32 * 1024 ||
        typeof value.prompt !== "string" ||
        Buffer.byteLength(value.prompt) > 240 * 1024 ||
        !boundedInteger(value.max_output_tokens, MAX_OUTPUT_TOKENS) ||
        !boundedInteger(value.max_output_bytes, MAX_OUTPUT_BYTES) ||
        !boundedInteger(value.max_duration_ms, MAX_DURATION_MS)
    ) {
        throw new Error("Invalid native capture work response");
    }
    return {
        lease: value.lease,
        work: {
            model: value.model,
            system: value.system,
            prompt: value.prompt,
            maxOutputTokens: value.max_output_tokens,
            maxOutputBytes: value.max_output_bytes,
            maxDurationMs: value.max_duration_ms,
        },
    };
}

/** Bounds one flush to a fixed number of daemon exchanges; the next flush resumes the rest. */
const FLUSH_MAX_BATCHES = 32;

/**
 * `"pending"` means the daemon retains work for a later flush: a `pending` or
 * `stale` reply, or the batch bound reached with work still flowing.
 * Only `store_failed`, `unavailable`, and malformed replies reject.
 */
export type MemoryCaptureFlushResult = "ready" | "disabled" | "pending";

function flushOutcome(state: string | undefined): MemoryCaptureFlushResult | "work" | "failed" {
    switch (state) {
        case "ready":
        case "disabled":
        case "work":
            return state;
        case "pending":
        case "stale":
            return "pending";
        default:
            return "failed";
    }
}

/** Native auth/model execution stays in the harness. Only bounded proposals go
 * back to the daemon; its confirmed receipts, not model success, finish capture. */
export async function flushMemoryCapture(
    client: Pick<RustModeModuleClient, "call">,
    input: { sessionId: string; projectRoot: string; model?: string },
    execute: NativeCaptureExecutor,
    options: { signal?: AbortSignal } = {},
): Promise<MemoryCaptureFlushResult> {
    const { signal } = options;
    const call = (
        method: "memory.capture.next" | "memory.capture.submit",
        extra: Record<string, unknown> = {},
    ) =>
        client.call({
            sessionId: input.sessionId,
            projectRoot: input.projectRoot,
            method,
            timeoutMs: 30_000,
            body: {
                method,
                v: 2,
                session_id: input.sessionId,
                project_root: input.projectRoot,
                ...extra,
            },
        });
    const unfinished = () =>
        new Error("Memory capture has unfinished work; durable facts are not yet confirmed saved.");
    const cancelled = () => new NativeCaptureError("cancelled");
    // Losing this notification leaves an expiring lease, not a successful capture.
    // An aborted flush makes no daemon call at all; the daemon expires the held lease.
    const release = (lease: string, code: NativeCaptureError["code"]) =>
        signal?.aborted
            ? Promise.resolve()
            : withTimeout(
                  call("memory.capture.submit", { lease, error: code }),
                  HOST_SDK_READ_TIMEOUT_MS,
                  "capture failure notification timed out",
              ).catch(() => undefined);
    for (let batch = 0; batch < FLUSH_MAX_BATCHES; batch++) {
        if (signal?.aborted) throw cancelled();
        const response = await call("memory.capture.next", { model: input.model });
        const outcome = flushOutcome(stateOf(response));
        if (outcome === "failed") throw unfinished();
        if (outcome !== "work") return outcome;
        let lease: string;
        let work: NativeCaptureWork;
        try {
            ({ lease, work } = nativeWork(response));
        } catch (error) {
            // The daemon reserved this lease; without a release it stays held until expiry.
            const reserved = leaseOf(response);
            if (reserved !== undefined) await release(reserved, "cancelled");
            throw error;
        }
        if (signal?.aborted) throw cancelled();
        const controller = new AbortController();
        const abort = () => controller.abort();
        signal?.addEventListener("abort", abort, { once: true });
        let result: { model: string; text: string };
        try {
            result = await withTimeout(
                execute(work, controller.signal),
                work.maxDurationMs,
                "native capture timed out",
            );
            if (result.model !== work.model || typeof result.text !== "string")
                throw new NativeCaptureError("model_failed");
            if (Buffer.byteLength(result.text) > work.maxOutputBytes)
                throw new NativeCaptureError("output_limit");
        } catch (error) {
            controller.abort();
            const code = error instanceof NativeCaptureError ? error.code : "model_failed";
            await release(lease, code);
            throw new NativeCaptureError(code);
        } finally {
            signal?.removeEventListener("abort", abort);
        }
        if (signal?.aborted) throw cancelled();
        const submitted = await call("memory.capture.submit", {
            lease,
            model: result.model,
            output: result.text,
        });
        const receipt = stateOf(submitted);
        if (receipt === "processed") continue;
        if (receipt === "pending" || receipt === "stale") return "pending";
        throw unfinished();
    }
    return "pending";
}

export interface MemoryCaptureScope {
    sessionId: string;
    projectRoot: string;
    model?: string;
}

export interface MemoryCaptureDrainHooks {
    /** Receives every settled pass, including quiet `"pending"` outcomes. */
    onSettled(scope: MemoryCaptureScope, result: MemoryCaptureFlushResult): void;
    /** Receives only failures thrown by `flushMemoryCapture`. */
    onFailed(scope: MemoryCaptureScope, error: unknown): void;
}

export interface MemoryCaptureDrain {
    /** Starts a drain for `scope.projectRoot`, or marks one rerun if a drain is already running. */
    schedule(scope: MemoryCaptureScope): void;
    /** Settles once the running drain and its queued rerun finish; `undefined` when idle. */
    pending(projectRoot: string): Promise<void> | undefined;
    /** Settles once every project is idle, including drains scheduled while waiting. */
    settle(): Promise<void>;
    /** Called by the owner before it disconnects `client`; a closed drain makes no further daemon call. */
    close(): void;
}

/** One drain per project root runs at a time; a rerun uses the latest scope. A drain never rejects. */
export function createMemoryCaptureDrain(
    client: Pick<RustModeModuleClient, "call">,
    execute: NativeCaptureExecutor,
    hooks: MemoryCaptureDrainHooks,
): MemoryCaptureDrain {
    interface Running {
        done: Promise<void>;
        rerun?: MemoryCaptureScope;
        controller: AbortController;
    }
    const running = new Map<string, Running>();
    let closed = false;
    function notify(report: () => void): void {
        try {
            report();
        } catch {
            // A failing hook must not stop the queued rerun.
        }
    }
    async function drain(entry: Running, first: MemoryCaptureScope): Promise<void> {
        let scope = first;
        try {
            for (;;) {
                let result: MemoryCaptureFlushResult | undefined;
                let failure: unknown;
                try {
                    result = await flushMemoryCapture(client, scope, execute, {
                        signal: entry.controller.signal,
                    });
                } catch (error) {
                    failure = error;
                }
                // A closed drain has no owner left to report to.
                if (closed) return;
                if (result !== undefined) notify(() => hooks.onSettled(scope, result));
                else notify(() => hooks.onFailed(scope, failure));
                const next = entry.rerun;
                entry.rerun = undefined;
                if (!next) return;
                scope = next;
            }
        } finally {
            running.delete(first.projectRoot);
        }
    }
    return {
        schedule(scope) {
            if (closed) return;
            const current = running.get(scope.projectRoot);
            if (current) {
                current.rerun = scope;
                return;
            }
            const entry: Running = { done: Promise.resolve(), controller: new AbortController() };
            running.set(scope.projectRoot, entry);
            entry.done = drain(entry, scope);
        },
        pending(projectRoot) {
            return running.get(projectRoot)?.done;
        },
        async settle() {
            while (running.size > 0)
                await Promise.all([...running.values()].map((entry) => entry.done));
        },
        close() {
            closed = true;
            for (const entry of running.values()) {
                entry.rerun = undefined;
                entry.controller.abort();
            }
        },
    };
}

/** The cache saves repeat uploads only. Daemon receipts, not this cache, own
 * durability and replay. Failures leave entries unacknowledged for next time. */
export function createMemoryCaptureCheckpoint(client: Pick<RustModeModuleClient, "call">) {
    const acknowledged = new Map<string, string>();
    return async (input: {
        sessionId: string;
        projectRoot: string;
        model?: string;
        messages: Iterable<CaptureMessage>;
    }): Promise<void> => {
        const prefix = `${input.projectRoot}\0${input.sessionId}\0`;
        let batch: CaptureMessage[] = [];
        let batchKeys: Array<[string, string]> = [];
        let bytes = 0;
        async function send(): Promise<void> {
            if (batch.length === 0) return;
            const response = await client.call({
                sessionId: input.sessionId,
                projectRoot: input.projectRoot,
                method: "memory.capture",
                body: {
                    method: "memory.capture",
                    v: 2,
                    session_id: input.sessionId,
                    project_root: input.projectRoot,
                    model: input.model,
                    messages: batch,
                },
            });
            const state = stateOf(response);
            if (state !== "accepted") {
                throw new Error(
                    `Memory capture checkpoint not accepted (${state === "disabled" || state === "queue_full" || state === "store_failed" || state === "project_mismatch" ? state : "invalid_response"})`,
                );
            }
            for (const [key, digest] of batchKeys) {
                acknowledged.set(key, digest);
                if (acknowledged.size > ACK_CACHE_ENTRIES) {
                    const oldest = acknowledged.keys().next().value;
                    if (oldest !== undefined) acknowledged.delete(oldest);
                }
            }
            batch = [];
            batchKeys = [];
            bytes = 0;
        }
        for (const message of input.messages) {
            for (const fragment of captureFragments(message)) {
                const key = `${prefix}${fragment.id}`;
                const digest = createHash("sha256")
                    .update(fragment.role)
                    .update("\0")
                    .update(fragment.text)
                    .digest("hex");
                if (acknowledged.get(key) === digest) continue;
                const size = Buffer.byteLength(fragment.text);
                if (batch.length >= BATCH_MESSAGES || bytes + size > BATCH_BYTES) await send();
                batch.push(fragment);
                batchKeys.push([key, digest]);
                bytes += size;
            }
        }
        await send();
    };
}
