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

/** Native text only: no tool results, reasoning, synthetic summaries, or failed responses. */
export function* piCaptureMessages(entries: Iterable<unknown>): Generator<CaptureMessage> {
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

/** Complete OpenCode conversation records; ignored/synthetic text is not source evidence. */
export function* openCodeCaptureMessages(messages: Iterable<unknown>): Generator<CaptureMessage> {
    for (const value of messages) {
        const message = record(value);
        const info = record(message?.info);
        if (!info || typeof info.id !== "string" || !Array.isArray(message?.parts)) continue;
        if (info.role !== "user" && info.role !== "assistant") continue;
        if (info.summary === true || info.error !== undefined) continue;
        if (info.role === "assistant" && typeof record(info.time)?.completed !== "number") continue;
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

function nativeWork(response: unknown): { lease: string; work: NativeCaptureWork } {
    const value = record(response);
    if (
        !value ||
        typeof value.lease !== "string" ||
        !/^[a-f0-9]{32}$/.test(value.lease) ||
        typeof value.model !== "string" ||
        !value.model.includes("/") ||
        value.model.length > 512 ||
        typeof value.system !== "string" ||
        Buffer.byteLength(value.system) > 32 * 1024 ||
        typeof value.prompt !== "string" ||
        Buffer.byteLength(value.prompt) > 240 * 1024 ||
        value.max_output_tokens !== 8192 ||
        value.max_output_bytes !== 128 * 1024 ||
        value.max_duration_ms !== 90_000
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

/** Native auth/model execution stays in the harness. Only bounded proposals go
 * back to the daemon; its confirmed receipts, not model success, finish capture. */
export async function flushMemoryCapture(
    client: Pick<RustModeModuleClient, "call">,
    input: { sessionId: string; projectRoot: string; model?: string },
    execute: NativeCaptureExecutor,
): Promise<void> {
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
    for (let batch = 0; batch < 32; batch++) {
        const response = await call("memory.capture.next", { model: input.model });
        const state = stateOf(response);
        if (state === "ready" || state === "disabled") return;
        if (state !== "work") break;
        const { lease, work } = nativeWork(response);
        const controller = new AbortController();
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
            // Losing this notification leaves an expiring lease, not a successful capture.
            await withTimeout(
                call("memory.capture.submit", { lease, error: code }),
                HOST_SDK_READ_TIMEOUT_MS,
                "capture failure notification timed out",
            ).catch(() => undefined);
            throw new NativeCaptureError(code);
        }
        const submitted = await call("memory.capture.submit", {
            lease,
            model: result.model,
            output: result.text,
        });
        if (stateOf(submitted) !== "processed") break;
    }
    throw new Error(
        "Memory capture has unfinished work; durable facts are not yet confirmed saved.",
    );
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
