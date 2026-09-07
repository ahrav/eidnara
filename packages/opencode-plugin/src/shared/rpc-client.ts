import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import {
    isPidAlive,
    isPidIdentityPlausible,
    legacyRpcPortFilePath,
    parseRpcPortFile,
    type RpcPortFileRecord,
    rpcPortDir,
} from "./rpc-utils";

const MAX_RETRIES = 10;
const RETRY_DELAY_MS = 500;
const REQUEST_TIMEOUT_MS = 5000;
const MAX_RERESOLVE_ATTEMPTS = 3;
const NON_RETRYABLE_RPC_ERROR = Symbol("nonRetryableRpcError");
type NonRetryableRpcError = Error & { [NON_RETRYABLE_RPC_ERROR]: true };

export interface EidnaraRpcClientOptions {
    /** Deadline for one HTTP exchange, headers and body included. */
    requestTimeoutMs?: number;
    /** Pause between discovery passes while waiting for a server to appear. */
    retryDelayMs?: number;
}

export class EidnaraRpcClient {
    private port: number | null = null;
    private pid: number | null = null;
    private token: string | null = null;
    private instanceId: string | null = null;
    private portDir: string;
    private legacyPortFilePath: string;
    private healthChecked = false;
    private readonly requestTimeoutMs: number;
    private readonly retryDelayMs: number;

    constructor(storageDir: string, directory: string, options: EidnaraRpcClientOptions = {}) {
        this.portDir = rpcPortDir(storageDir, directory);
        this.legacyPortFilePath = legacyRpcPortFilePath(storageDir, directory);
        this.requestTimeoutMs = options.requestTimeoutMs ?? REQUEST_TIMEOUT_MS;
        this.retryDelayMs = options.retryDelayMs ?? RETRY_DELAY_MS;
    }

    /* */
    async call<T = Record<string, unknown>>(
        method: string,
        params: Record<string, unknown> = {},
    ): Promise<T> {
        let lastError: unknown = null;

        for (let attempt = 0; attempt < MAX_RERESOLVE_ATTEMPTS; attempt++) {
            const port = await this.resolvePort();
            if (!port) {
                // `resolvePort` owns the wait for a server to appear; this loop only re-resolves
                // after a call to a resolved server fails.
                lastError = new Error("Eidnara RPC server not available");
                this.reset();
                break;
            }

            try {
                const response = await this.fetchWithTimeout(
                    `http://127.0.0.1:${port}/rpc/${method}`,
                    {
                        method: "POST",
                        headers: {
                            "Content-Type": "application/json",
                            // The server requires this per-process token on all
                            // non-health calls; read from the same port file used
                            // for discovery. Older servers wrote no token — send
                            // nothing then (they also require nothing).
                            ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}),
                        },
                        body: JSON.stringify(params),
                    },
                );

                if (!response.ok) {
                    const error = new Error(
                        `RPC ${method} failed (${response.status}): ${response.body}`,
                    );
                    if (response.status === 401 || response.status >= 500) {
                        lastError = error;
                        this.reset();
                        continue;
                    }
                    (error as NonRetryableRpcError)[NON_RETRYABLE_RPC_ERROR] = true;
                    throw error;
                }

                return JSON.parse(response.body) as T;
            } catch (err) {
                if (isNonRetryableRpcError(err)) {
                    throw err;
                }
                lastError = err;
                this.reset();
            }
        }

        if (lastError instanceof Error) {
            throw lastError;
        }
        throw new Error("Eidnara RPC server not available");
    }

    /* */
    async isAvailable(): Promise<boolean> {
        try {
            const port = await this.resolvePort();
            return port !== null;
        } catch {
            return false;
        }
    }

    /** Resolve the live server's port + bearer token (for opening the WS push
     *  channel). Reuses the same health-checked port-file discovery as `call`,
     * so the WS client and HTTP client use the same discovery and health-check rules.
     * */
    async resolveEndpoint(): Promise<{
        port: number;
        token: string | null;
        instanceId: string | null;
    } | null> {
        try {
            // The socket owns reconnect backoff, so endpoint discovery performs one
            // filesystem/health pass instead of nesting the HTTP client's retries.
            const port = await this.resolvePort(1);
            if (port === null) return null;
            return { port, token: this.token, instanceId: this.instanceId };
        } catch {
            return null;
        }
    }

    private async resolvePort(maxAttempts = MAX_RETRIES): Promise<number | null> {
        if (this.port && this.healthChecked) {
            // Reuse the cached port unless its recorded pid is known dead; a freed localhost
            // port can be rebound by another local process.
            if (this.pid === null || isPidAlive(this.pid) !== "dead") return this.port;
            this.reset();
        }

        for (let attempt = 0; attempt < maxAttempts; attempt++) {
            for (const record of this.readPortFiles()) {
                if (!(await this.healthCheck(record))) continue;
                this.port = record.port;
                this.pid = record.pid;
                this.token = record.token ?? null;
                this.instanceId = record.instance_id ?? null;
                this.healthChecked = true;
                return record.port;
            }

            this.reset();
            if (attempt < maxAttempts - 1) {
                await new Promise((resolve) => setTimeout(resolve, this.retryDelayMs));
            }
        }

        return null;
    }

    private readPortFiles(): RpcPortFileRecord[] {
        const records: RpcPortFileRecord[] = [];

        try {
            for (const entry of readdirSync(this.portDir)) {
                if (!entry.startsWith("port-") || !entry.endsWith(".json")) continue;
                const record = parseRpcPortFile(readFileSync(join(this.portDir, entry), "utf-8"));
                if (record && isDiscoveryCandidate(record)) records.push(record);
            }
        } catch {
            // Directory may not exist yet. Fall back to the legacy file below.
        }

        try {
            const legacy = parseRpcPortFile(readFileSync(this.legacyPortFilePath, "utf-8"));
            if (legacy && isDiscoveryCandidate(legacy)) records.push(legacy);
        } catch {
            // Absence of the legacy port file does not prevent discovery.
        }

        // Discovery prefers the current process's server before another live OpenCode instance for the project.
        records.sort((a, b) => {
            const aLocal = a.pid === process.pid ? 1 : 0;
            const bLocal = b.pid === process.pid ? 1 : 0;
            return bLocal - aLocal || b.started_at - a.started_at;
        });
        return records;
    }

    private async healthCheck(record: RpcPortFileRecord): Promise<boolean> {
        try {
            const response = await this.fetchWithTimeout(`http://127.0.0.1:${record.port}/health`, {
                method: "GET",
            });
            if (!response.ok) return false;
            const body = JSON.parse(response.body) as { pid?: unknown; instance_id?: unknown };
            if (body.pid !== record.pid) return false;
            // v0.32 health responses omit instance IDs; the health check accepts a missing ID and requires any present ID to match.
            return (
                body.instance_id === undefined ||
                record.instance_id === undefined ||
                body.instance_id === record.instance_id
            );
        } catch {
            return false;
        }
    }

    /**
     * The body is read before the timer is cleared so the deadline covers the whole
     * exchange; `fetch` alone settles once headers arrive.
     */
    private async fetchWithTimeout(
        url: string,
        options: RequestInit,
    ): Promise<{ ok: boolean; status: number; body: string }> {
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), this.requestTimeoutMs);
        try {
            const response = await fetch(url, { ...options, signal: controller.signal });
            return { ok: response.ok, status: response.status, body: await response.text() };
        } finally {
            clearTimeout(timeout);
        }
    }

    reset(): void {
        this.port = null;
        this.pid = null;
        this.token = null;
        this.instanceId = null;
        this.healthChecked = false;
    }
}

/**
 * A record whose pid is known dead, or whose pid cannot have written it (pid reuse after a
 * crash, or a legacy plain-number file with no pid at all), is never probed: the health
 * check alone cannot tell the recorded server from another process bound to the same port.
 * Inconclusive probes keep the record; the health check then decides.
 */
function isDiscoveryCandidate(record: RpcPortFileRecord): boolean {
    return isPidAlive(record.pid) !== "dead" && isPidIdentityPlausible(record) !== "implausible";
}

function isNonRetryableRpcError(err: unknown): err is NonRetryableRpcError {
    return typeof err === "object" && err !== null && NON_RETRYABLE_RPC_ERROR in err;
}
