import { closeSync, openSync, writeSync } from "node:fs";

export interface PiTierOptions {
    messages: number;
    window: number;
    seed: number;
    cwd: string;
}

const BASE_MS = Date.UTC(2026, 0, 1);
const WINDOW_TEXT_BYTES = 5 * 1024;
const SHAPES = 6;
const MIN_WINDOW = 60;

function rng(seed: number): () => number {
    let state = seed >>> 0 || 0x9e3779b9;
    return () => {
        state ^= state << 13;
        state >>>= 0;
        state ^= state >>> 17;
        state ^= state << 5;
        state >>>= 0;
        return state;
    };
}

const WORDS = ["fold", "cache", "segment", "window", "tier", "budget", "branch", "entry"];

function text(next: () => number, bytes: number, label: string): string {
    const words: string[] = [label];
    let length = label.length;
    while (length < bytes) {
        const word = WORDS[next() % WORDS.length] as string;
        words.push(word);
        length += word.length + 1;
    }
    return words.join(" ").slice(0, Math.max(bytes, label.length));
}

const ZERO_USAGE = {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

function assistant(content: unknown[], stopReason: string, timestamp: number) {
    return {
        role: "assistant",
        content,
        api: "anthropic-messages",
        provider: "anthropic",
        model: "claude-sonnet-4-5",
        usage: ZERO_USAGE,
        stopReason,
        timestamp,
    };
}

/** `n` is a 1-based message position. */
function message(n: number, body: string, timestamp: number): Record<string, unknown> {
    const call = `call-${Math.ceil(n / SHAPES)}`;
    switch (n % SHAPES) {
        case 1:
            return { role: "user", content: [{ type: "text", text: body }], timestamp };
        case 2:
            return assistant(
                [
                    { type: "text", text: body },
                    {
                        type: "toolCall",
                        id: call,
                        name: "read",
                        arguments: { path: `src/${n}.ts` },
                    },
                ],
                "toolUse",
                timestamp,
            );
        case 3:
            return {
                role: "toolResult",
                toolCallId: call,
                toolName: "read",
                content: [{ type: "text", text: body }],
                isError: false,
                timestamp,
            };
        case 4:
            return assistant([{ type: "text", text: body }], "stop", timestamp);
        case 5:
            return {
                role: "bashExecution",
                command: `ls ${n}`,
                output: body,
                exitCode: 0,
                cancelled: false,
                truncated: false,
                timestamp,
            };
        default:
            return assistant(
                [
                    { type: "thinking", thinking: `considering ${n}` },
                    { type: "text", text: body },
                ],
                "stop",
                timestamp,
            );
    }
}

function entry(id: string, parentId: string | null, timestamp: number, message: unknown): string {
    return JSON.stringify({
        type: "message",
        id,
        parentId,
        timestamp: new Date(timestamp).toISOString(),
        message,
    });
}

/** `retryPosition` returns the 1-based position of a plain assistant reply in the window's first half. */
export function retryPosition(messages: number, window: number): number {
    const start = messages - window + Math.floor(window / 2);
    return start - ((start - 4 + SHAPES) % SHAPES);
}

export function writePiTier(path: string, options: PiTierOptions): string {
    const { messages, window, seed, cwd } = options;
    if (!Number.isSafeInteger(messages) || messages < window || window < MIN_WINDOW)
        throw new Error(`a tier needs a window of at least ${MIN_WINDOW} messages`);
    const next = rng(seed);
    const sessionId = `pi-tier-${seed.toString(16)}-${messages}`;
    const retry = retryPosition(messages, window);
    const fd = openSync(path, "w", 0o600);
    const lines: string[] = [];
    const flush = () => {
        if (lines.length === 0) return;
        writeSync(fd, `${lines.join("\n")}\n`);
        lines.length = 0;
    };
    try {
        lines.push(
            JSON.stringify({
                type: "session",
                version: 3,
                id: sessionId,
                timestamp: new Date(BASE_MS).toISOString(),
                cwd,
            }),
        );
        let parentId: string | null = null;
        for (let n = 1; n <= messages; n += 1) {
            const timestamp = BASE_MS + n * 1_000;
            const body = text(next, n > messages - window ? WINDOW_TEXT_BYTES : 24, `m${n}`);
            if (n === retry) {
                const failed = {
                    ...assistant([], "error", timestamp - 500),
                    errorMessage: "overloaded",
                };
                lines.push(entry("failed-1", parentId, timestamp - 500, failed));
                parentId = "failed-1";
            }
            lines.push(entry(`m${n}`, parentId, timestamp, message(n, body, timestamp)));
            parentId = `m${n}`;
            if (lines.length >= 4_096) flush();
        }
        flush();
    } finally {
        closeSync(fd);
    }
    return sessionId;
}
