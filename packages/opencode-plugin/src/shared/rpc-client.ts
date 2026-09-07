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

export interface EidnaraRpcClientOptions {
    /** Deadline for one HTTP exchange, headers and body included. */
    requestTimeoutMs?: number;
    /** Pause between discovery passes while waiting for a server to appear. */
    retryDelayMs?: number;
}

export class EidnaraRpcClient {
    /** The client caches the most recent record that passed identity and health checks, so a repeat call skips the directory scan. */
    private server: RpcPortFileRecord | null = null;
    private portDir: string;
    private legacyPortFilePath: string;
    private readonly requestTimeoutMs: number;
    private readonly retryDelayMs: number;

    constructor(storageDir: string, directory: string, options: EidnaraRpcClientOptions = {}) {
        this.portDir = rpcPortDir(storageDir, directory);
        this.legacyPortFilePath = legacyRpcPortFilePath(storageDir, directory);
        this.requestTimeoutMs = options.requestTimeoutMs ?? REQUEST_TIMEOUT_MS;
        this.retryDelayMs = options.retryDelayMs ?? RETRY_DELAY_MS;
    }

    /**
     * A request is sent at most once per resolved server. A 401 re-resolves to use a rotated port-file token. commentlint: allow(JUDGE)
     * On fetch failure, the client clears its cached record and does not retry because the handler may have run. commentlint: allow(JUDGE)
     */
    async call<T = Record<string, unknown>>(
        method: string,
        params: Record<string, unknown> = {},
    ): Promise<T> {
        let lastError: Error | null = null;

        for (let attempt = 0; attempt < MAX_RERESOLVE_ATTEMPTS; attempt++) {
            const server = await this.resolveServer();
            if (!server) {
                // `resolveServer` owns the wait for a server to appear. The loop re-resolves only when a resolved server rejects a call. commentlint: allow(JUDGE)
                throw lastError ?? new Error("Eidnara RPC server not available");
            }

            let response: { ok: boolean; status: number; body: string };
            try {
                response = await this.fetchWithTimeout(
                    `http://127.0.0.1:${server.port}/rpc/${method}`,
                    {
                        method: "POST",
                        headers: {
                            "Content-Type": "application/json",
                            // The server requires this per-process token on all
                            // non-health calls; read from the same port file used
                            // for discovery. Older servers wrote no token — send
                            // nothing then (they also require nothing).
                            ...(server.token ? { Authorization: `Bearer ${server.token}` } : {}),
                        },
                        body: JSON.stringify(params),
                    },
                );
            } catch (err) {
                this.reset();
                throw err;
            }

            if (response.status === 401) {
                lastError = new Error(`RPC ${method} failed (401): ${response.body}`);
                this.reset();
                continue;
            }
            if (!response.ok) {
                throw new Error(`RPC ${method} failed (${response.status}): ${response.body}`);
            }

            return JSON.parse(response.body) as T;
        }

        throw lastError ?? new Error("Eidnara RPC server not available");
    }

    /* */
    async isAvailable(): Promise<boolean> {
        try {
            return (await this.resolveServer()) !== null;
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
            const server = await this.resolveServer(1);
            if (server === null) return null;
            return {
                port: server.port,
                token: server.token ?? null,
                instanceId: server.instance_id ?? null,
            };
        } catch {
            return null;
        }
    }

    private async resolveServer(maxAttempts = MAX_RETRIES): Promise<RpcPortFileRecord | null> {
        if (this.server) {
            // A cached record passes the same gate as a freshly read one; the cache skips only the directory scan. A server replaced inside the same process keeps the pid alive but stops answering `/health` on the old port. commentlint: allow(JUDGE)
            if (isDiscoveryCandidate(this.server) && (await this.healthCheck(this.server))) {
                return this.server;
            }
            this.reset();
        }

        for (let attempt = 0; attempt < maxAttempts; attempt++) {
            for (const record of this.readPortFiles()) {
                if (!(await this.healthCheck(record))) continue;
                this.server = record;
                return record;
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
                const record = readPortFileRecord(join(this.portDir, entry));
                if (record && isDiscoveryCandidate(record)) records.push(record);
            }
        } catch {
            // Directory may not exist yet. Fall back to the legacy file below.
        }

        const legacy = readPortFileRecord(this.legacyPortFilePath);
        if (legacy && isDiscoveryCandidate(legacy)) records.push(legacy);

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
        this.server = null;
    }
}

/**
 * A port file can disappear between `readdirSync` and `readFileSync`; returning `null` lets the
 * caller scan the remaining candidates. Unreadable files, including permission-denied paths and
 * directories named like port files, return `null` for the same reason.
 */
function readPortFileRecord(path: string): RpcPortFileRecord | null {
    try {
        return parseRpcPortFile(readFileSync(path, "utf-8"));
    } catch {
        return null;
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
