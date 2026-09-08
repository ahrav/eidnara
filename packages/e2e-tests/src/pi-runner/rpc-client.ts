/**
 * Pi's `--mode rpc` protocol uses JSONL over stdio.
 * Pi accepts only LF-delimited JSON objects on stdin.
 * Pi identifies each `type: "response"` object with the caller-provided `id`.
 * Pi can emit `agent_start`, `agent_end`, `message_end`, and extension UI requests asynchronously.
 * `PiTestHarness` keeps one Pi subprocess alive for its entire lifetime.
 * `PiTestHarness` matches command responses by `id` and collects events per turn.
 * The client avoids Node's `readline` because `readline` treats U+2028 and U+2029 as line separators.
 * JSON strings can contain U+2028 and U+2029.
 */

import { type ChildProcess, spawn } from "node:child_process";
import { StringDecoder } from "node:string_decoder";
import {
    childEnv,
    createPiIsolatedEnv,
    PI_CLI,
    type PiIsolatedEnv,
    type PiRunnerOptions,
    writeConfigs,
} from "./spawn";

export type PiRpcEvent = Record<string, unknown> & { type?: string };

export interface PiRpcResponse<T = unknown> {
    id?: string;
    type: "response";
    command: string;
    success: boolean;
    data?: T;
    error?: string;
}

export interface PiRpcWaitOptions {
    timeoutMs?: number;
    label?: string;
}

interface PendingRequest {
    method: string;
    resolve: (response: PiRpcResponse) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
}

interface PendingWait {
    fail: (error: Error) => void;
}

export function serializeRpcMessage(value: Record<string, unknown>): string {
    return `${JSON.stringify(value)}\n`;
}

export function attachStrictJsonlReader(
    stream: NodeJS.ReadableStream,
    onLine: (line: string) => void,
): () => void {
    const decoder = new StringDecoder("utf8");
    let buffer = "";
    const emitLine = (line: string) => {
        onLine(line.endsWith("\r") ? line.slice(0, -1) : line);
    };
    const onData = (chunk: Buffer | string) => {
        buffer += typeof chunk === "string" ? chunk : decoder.write(chunk);
        while (true) {
            const newlineIndex = buffer.indexOf("\n");
            if (newlineIndex === -1) return;
            emitLine(buffer.slice(0, newlineIndex));
            buffer = buffer.slice(newlineIndex + 1);
        }
    };
    const onEnd = () => {
        buffer += decoder.end();
        if (buffer.length > 0) {
            emitLine(buffer);
            buffer = "";
        }
    };
    stream.on("data", onData);
    stream.on("end", onEnd);
    return () => {
        stream.off("data", onData);
        stream.off("end", onEnd);
    };
}

export class PiRpcProtocol {
    private nextId = 0;
    private readonly pendingRequests = new Map<string, PendingRequest>();
    private readonly pendingWaits = new Set<PendingWait>();
    private readonly eventListeners = new Set<(event: PiRpcEvent) => void>();

    onEvent(listener: (event: PiRpcEvent) => void): () => void {
        this.eventListeners.add(listener);
        return () => this.eventListeners.delete(listener);
    }

    sendCommand<T = unknown>(
        writeLine: (line: string) => void,
        method: string,
        params: Record<string, unknown> = {},
        opts: PiRpcWaitOptions = {},
    ): Promise<PiRpcResponse<T>> {
        const id = `pi-e2e-${++this.nextId}`;
        const timeoutMs = opts.timeoutMs ?? 30_000;
        const command = { ...params, id, type: method };
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
                this.pendingRequests.delete(id);
                reject(
                    new Error(
                        `Timed out after ${timeoutMs}ms waiting for Pi RPC response to ${method}`,
                    ),
                );
            }, timeoutMs);
            this.pendingRequests.set(id, {
                method,
                timer,
                resolve: (response) => resolve(response as PiRpcResponse<T>),
                reject,
            });
            try {
                writeLine(serializeRpcMessage(command));
            } catch (error) {
                clearTimeout(timer);
                this.pendingRequests.delete(id);
                reject(error instanceof Error ? error : new Error(String(error)));
            }
        });
    }

    waitForEvent(
        predicate: (event: PiRpcEvent) => boolean,
        opts: PiRpcWaitOptions = {},
    ): Promise<PiRpcEvent> {
        const timeoutMs = opts.timeoutMs ?? 60_000;
        const label = opts.label ? ` (${opts.label})` : "";
        return new Promise((resolve, reject) => {
            const wait: PendingWait = {
                fail: (error) => {
                    settle();
                    reject(error);
                },
            };
            const settle = () => {
                clearTimeout(timer);
                unsubscribe();
                this.pendingWaits.delete(wait);
            };
            const unsubscribe = this.onEvent((event) => {
                try {
                    if (!predicate(event)) return;
                    settle();
                    resolve(event);
                } catch (error) {
                    wait.fail(error instanceof Error ? error : new Error(String(error)));
                }
            });
            const timer = setTimeout(() => {
                wait.fail(
                    new Error(`Timed out after ${timeoutMs}ms waiting for Pi RPC event${label}`),
                );
            }, timeoutMs);
            this.pendingWaits.add(wait);
        });
    }

    dispatchLine(line: string): void {
        let message: PiRpcEvent | PiRpcResponse;
        try {
            message = JSON.parse(line) as PiRpcEvent | PiRpcResponse;
        } catch (error) {
            this.dispatchEvent({
                type: "rpc_parse_error",
                line,
                error: error instanceof Error ? error.message : String(error),
            });
            return;
        }

        if (message.type === "response") {
            const id = typeof message.id === "string" ? message.id : undefined;
            const pending = id === undefined ? undefined : this.pendingRequests.get(id);
            if (id !== undefined && pending) {
                clearTimeout(pending.timer);
                this.pendingRequests.delete(id);
                pending.resolve(message as PiRpcResponse);
                return;
            }
        }

        this.dispatchEvent(message as PiRpcEvent);
    }

    /** Fails every outstanding command and event wait; a dead process can answer neither. */
    rejectPending(error: Error): void {
        for (const [id, pending] of this.pendingRequests) {
            clearTimeout(pending.timer);
            pending.reject(error);
            this.pendingRequests.delete(id);
        }
        for (const wait of [...this.pendingWaits]) {
            wait.fail(error);
        }
    }

    private dispatchEvent(event: PiRpcEvent): void {
        for (const listener of [...this.eventListeners]) {
            listener(event);
        }
    }
}

export interface PiRpcClientOptions extends PiRunnerOptions {
    env?: PiIsolatedEnv;
}

export interface PiState extends Record<string, unknown> {
    sessionId?: string;
    sessionFile?: string;
    isStreaming?: boolean;
    isCompacting?: boolean;
    messageCount?: number;
}

export type PiMessage = Record<string, unknown>;
export type PiSessionStats = Record<string, unknown>;

export class PiRpcClient {
    readonly env: PiIsolatedEnv;
    readonly protocol = new PiRpcProtocol();

    private readonly options: PiRpcClientOptions;
    private process: ChildProcess | null = null;
    private stopReadingStdout: (() => void) | null = null;
    private stderr = "";
    /** Set once the child has closed; later commands fail with it instead of writing to a dead stdin. */
    private exitError: Error | null = null;

    constructor(options: PiRpcClientOptions) {
        this.env = options.env ?? createPiIsolatedEnv();
        this.options = { ...options, env: this.env };
    }

    async start(): Promise<void> {
        if (this.process) throw new Error("Pi RPC client already started");
        if (PI_CLI === null) {
            throw new Error("@earendil-works/pi-coding-agent is not installed; run bun install");
        }
        writeConfigs(this.env, this.options);

        // Pi's CLI is a Node ESM entrypoint, so the child runs under `node` rather than the Bun test process.
        const child = spawn(
            "node",
            [
                PI_CLI,
                "--mode",
                "rpc",
                "--no-extensions",
                "--extension",
                this.env.pluginDir,
                "--no-skills",
                "--no-prompt-templates",
                "--no-themes",
                "--model",
                "anthropic/claude-haiku-4-5",
                "--api-key",
                "test-key-not-real",
            ],
            { cwd: this.env.workdir, env: childEnv(this.env), stdio: ["pipe", "pipe", "pipe"] },
        );
        this.process = child;

        child.stderr?.on("data", (chunk: Buffer) => {
            this.stderr += chunk.toString();
        });
        if (!child.stdout) throw new Error("Pi RPC process has no stdout pipe");
        this.stopReadingStdout = attachStrictJsonlReader(child.stdout, (line) => {
            this.protocol.dispatchLine(line);
        });
        // `close` follows `exit` once the stdio pipes have drained, so an `agent_end` written just
        // before the process died reaches its waiter before the waiter is failed.
        child.once("close", (code, signal) => {
            const error = this.processExitError(code, signal);
            this.exitError = error;
            this.protocol.rejectPending(error);
        });

        await Bun.sleep(100);
        if (child.exitCode !== null) {
            throw new Error(
                `Pi RPC process exited during startup with code ${child.exitCode}\n${this.stderr}`,
            );
        }
    }

    onEvent(listener: (event: PiRpcEvent) => void): () => void {
        return this.protocol.onEvent(listener);
    }

    waitForEvent(
        predicate: (event: PiRpcEvent) => boolean,
        opts: PiRpcWaitOptions = {},
    ): Promise<PiRpcEvent> {
        return this.protocol.waitForEvent(predicate, opts);
    }

    async sendCommand<T = unknown>(
        method: string,
        params: Record<string, unknown> = {},
        opts: PiRpcWaitOptions = {},
    ): Promise<PiRpcResponse<T>> {
        const child = this.process;
        const stdin = child?.stdin;
        if (!child || !stdin || child.killed) {
            throw new Error("Pi RPC process is not running");
        }
        // `killed` only records kills sent through this client.
        if (child.exitCode !== null || child.signalCode !== null) {
            throw this.exitError ?? this.processExitError(child.exitCode, child.signalCode);
        }
        return this.protocol.sendCommand<T>((line) => stdin.write(line), method, params, opts);
    }

    getStderr(): string {
        return this.stderr;
    }

    /** Both fields are `null` while the child is alive; either one is set once it has exited. */
    processStatus(): { exitCode: number | null; signalCode: NodeJS.Signals | null } {
        return {
            exitCode: this.process?.exitCode ?? null,
            signalCode: this.process?.signalCode ?? null,
        };
    }

    private processExitError(code: number | null, signal: NodeJS.Signals | null): Error {
        return new Error(
            `Pi RPC process exited with code ${code ?? "null"} signal ${signal ?? "null"}\n${this.stderr}`,
        );
    }

    async shutdown(timeoutMs = 2_000): Promise<void> {
        if (!this.process) return;
        const child = this.process;
        this.stopReadingStdout?.();
        this.stopReadingStdout = null;
        this.process = null;

        if (child.exitCode !== null || child.signalCode !== null) return;
        child.kill("SIGTERM");
        await new Promise<void>((resolve) => {
            const timer = setTimeout(() => {
                if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
                resolve();
            }, timeoutMs);
            child.once("exit", () => {
                clearTimeout(timer);
                resolve();
            });
        });
    }
}

export function requireSuccessfulResponse<T>(response: PiRpcResponse<T>): T {
    if (!response.success) {
        throw new Error(response.error ?? `Pi RPC ${response.command} failed`);
    }
    return response.data as T;
}
