/**
 * Adapts `HostModuleTransport` to `KernelTransport` and creates per-session
 * clients that share a transport and token cache per connection file.
 */

import { existsSync } from "node:fs";
import {
    KernelClient,
    type KernelClientResolver,
    type KernelTransport,
    type KernelTransportCall,
    StoreLifecycleError,
    type StoreLifecycleReason,
    TokenCache,
} from "../../shared/kernel-client";
import { isRecord } from "../../shared/record-type-guard";
import { HostModuleTransport } from "./module-transport";
import type { KernelMethod } from "./module-wire";

const KERNEL_METHODS: ReadonlySet<string> = new Set<KernelMethod>([
    "kernel.read",
    "kernel.commit",
    "kernel.eligibility.batch",
    "kernel.egress.decide",
    "kernel.artifact.ingest.begin",
    "kernel.artifact.ingest.page",
    "kernel.artifact.ingest.finish",
]);

function isKernelMethod(method: string): method is KernelMethod {
    return KERNEL_METHODS.has(method);
}

const STORE_LIFECYCLE_REASONS: Readonly<Record<string, StoreLifecycleReason>> = {
    storage_starting: "store_starting",
    storage_unavailable: "store_unavailable",
};

function storeLifecycleReason(error: unknown): StoreLifecycleReason | undefined {
    return isRecord(error) && typeof error.code === "string"
        ? STORE_LIFECYCLE_REASONS[error.code]
        : undefined;
}

/** A demand-start-capable transport is reachable with no connection file because `call` starts its daemon; every other origin keeps the synchronous stat that answers `daemon_absent` before any dial. The daemon dispatches on the method encoded in the body, so `call` refuses a body whose `method` differs from the checked one; otherwise a permitted `args.method` could carry a non-kernel body to a destructive handler. commentlint: allow(JUDGE) */
export function createKernelTransport(transport: HostModuleTransport): KernelTransport {
    return {
        connectionFileExists: () =>
            transport.canDemandStart() || existsSync(transport.connectionFilePath),
        async call(args: KernelTransportCall): Promise<unknown> {
            if (!isKernelMethod(args.method)) {
                throw new Error(`kernel transport refuses non-kernel method ${args.method}`);
            }
            if (!isRecord(args.body) || args.body.method !== args.method) {
                throw new Error(
                    `kernel transport refuses a body whose encoded method is not ${args.method}`,
                );
            }
            try {
                return await transport.call({
                    sessionId: args.sessionId,
                    projectRoot: args.projectRoot,
                    method: args.method,
                    body: args.body,
                    ...(args.signal ? { signal: args.signal } : {}),
                    ...(args.timeoutMs === undefined ? {} : { timeoutMs: args.timeoutMs }),
                });
            } catch (error) {
                const reason = storeLifecycleReason(error);
                if (reason !== undefined) throw new StoreLifecycleError(reason);
                throw error;
            }
        },
        async ensureRoute(args): Promise<void> {
            transport.forgetRoute(args.sessionId, args.projectRoot);
        },
    };
}

/** The configuration slice the factory reads; `EidnaraConfig` satisfies it. */
export interface KernelClientConfig {
    memory?: { enabled?: boolean };
    subc?: { connection_file?: string };
}

interface SharedKernelState {
    module: HostModuleTransport;
    adapter: KernelTransport;
    /** What clients hold: resolves to the live state's adapter on every call, so a client that outlives this state's eviction follows the map to its replacement instead of redialing the evicted `module`. commentlint: allow(JUDGE) */
    transport: KernelTransport;
    tokens: TokenCache;
    /** The set orders project roots from least to most recently resolved and bounds per-project token buckets. */
    tokenProjectOrder: Set<string>;
}

/** Cap on project roots whose token buckets the shared cache retains per connection file; resolving a client past the cap evicts the least-recently-resolved root's tokens. commentlint: allow(JUDGE) */
export const MAX_TOKEN_CACHE_PROJECTS = 32;

/** Cap on connection files whose shared transports the process retains: a long-lived host that `/cd`s across projects with distinct `connection_file` values would otherwise accumulate one live transport — socket, token cache, route cache — per daemon configuration forever. Eviction disconnects the transport, so a call in flight on it fails as a transport failure; a client resolved before the eviction reaches the replacement state through its indirect transport on its next call, and the cap holds because that replacement is created through the same map. commentlint: allow(JUDGE) */
export const MAX_CONNECTION_FILE_STATES = 8;

/** Every kernel operation resolves a client for its project root first; client resolution order therefore tracks token-cache access order. commentlint: allow(JUDGE) */
function touchTokenProject(shared: SharedKernelState, projectRoot: string): void {
    shared.tokenProjectOrder.delete(projectRoot);
    shared.tokenProjectOrder.add(projectRoot);
    while (shared.tokenProjectOrder.size > MAX_TOKEN_CACHE_PROJECTS) {
        const oldest: string | undefined = shared.tokenProjectOrder.values().next().value;
        if (oldest === undefined) break;
        shared.tokenProjectOrder.delete(oldest);
        shared.tokens.dropProject(oldest);
    }
}

const sharedByConnectionFile = new Map<string, SharedKernelState>();

/** Tags the key so the managed default (`undefined`, may demand-start) never shares a state with an explicit empty path (`""`, never demand-starts); `resolveConnectionOrigin` distinguishes them by presence, not value. commentlint: allow(JUDGE) */
function connectionFileKey(connectionFile: string | undefined): string {
    return connectionFile === undefined ? "managed-default" : `explicit:${connectionFile}`;
}

function indirectTransport(
    connectionFile: string | undefined,
    adapter: KernelTransport,
): KernelTransport {
    const live = (): KernelTransport =>
        sharedByConnectionFile.get(connectionFileKey(connectionFile))?.adapter === adapter
            ? adapter
            : sharedState(connectionFile).adapter;
    return {
        connectionFileExists: () => live().connectionFileExists(),
        call: (args) => live().call(args),
        ensureRoute: (args) => live().ensureRoute(args),
    };
}

/** A token's `known_as_of` is a position in the evicted transport's daemon sequence, so the cache a retained client still holds must not carry those tokens to the replacement state's daemon; the client's next read refills it from that daemon. commentlint: allow(JUDGE) */
function evictSharedState(evicted: SharedKernelState): void {
    evicted.module.disconnect();
    for (const projectRoot of evicted.tokenProjectOrder) evicted.tokens.dropProject(projectRoot);
    evicted.tokenProjectOrder.clear();
}

function sharedState(connectionFile: string | undefined): SharedKernelState {
    const key = connectionFileKey(connectionFile);
    let shared = sharedByConnectionFile.get(key);
    if (shared) {
        // Map insertion order doubles as recency order, so a hit re-inserts its entry.
        sharedByConnectionFile.delete(key);
        sharedByConnectionFile.set(key, shared);
        return shared;
    }
    const module = new HostModuleTransport(connectionFile);
    const adapter = createKernelTransport(module);
    shared = {
        module,
        adapter,
        transport: indirectTransport(connectionFile, adapter),
        tokens: new TokenCache(),
        tokenProjectOrder: new Set(),
    };
    sharedByConnectionFile.set(key, shared);
    while (sharedByConnectionFile.size > MAX_CONNECTION_FILE_STATES) {
        const oldestKey: string | undefined = sharedByConnectionFile.keys().next().value;
        if (oldestKey === undefined) break;
        const evicted = sharedByConnectionFile.get(oldestKey);
        sharedByConnectionFile.delete(oldestKey);
        if (evicted) evictSharedState(evicted);
    }
    return shared;
}

/**
 * Closes one session's routes on every shared transport. A route is keyed by `(session, root)` and otherwise lives until its transport disconnects, so a long-lived host that cycles through sessions would hold one route per session it ever served. The next call for that session reopens its route. commentlint: allow(JUDGE)
 */
export function closeKernelSession(sessionId: string): void {
    for (const shared of sharedByConnectionFile.values()) {
        shared.module.closeSession(sessionId);
    }
}

export interface CreateKernelClientArgs {
    sessionId: string;
    projectRoot: string;
    config: KernelClientConfig;
    /** Replaces the shared module transport and opts out of the shared token cache: a token's `known_as_of` is a position in one daemon's event sequence, and tokens minted against one transport's daemon are not valid against another's. Without an explicit `tokens`, each call gets a fresh cache; pass `tokens` to keep mutation-token continuity across clients on the same transport. commentlint: allow(JUDGE) */
    transport?: KernelTransport;
    tokens?: TokenCache;
}

/** Applies `memory.enabled` to every client. Clients for the same connection file share a transport (one dial, one route cache) and a token cache (tokens are keyed by project, not session). Shared-path clients take the root the transport canonicalizes, so a symlinked and a resolved spelling of one project derive the same operation keys and token bucket as the route they are bound to. commentlint: allow(JUDGE) */
export function createKernelClient(args: CreateKernelClientArgs): KernelClient {
    const shared = args.transport ? null : sharedState(args.config.subc?.connection_file);
    const projectRoot = shared ? shared.module.canonicalRoot(args.projectRoot) : args.projectRoot;
    if (shared && args.tokens === undefined) {
        touchTokenProject(shared, projectRoot);
    }
    return new KernelClient({
        transport: args.transport ?? (shared as SharedKernelState).transport,
        tokens: args.tokens ?? shared?.tokens ?? new TokenCache(),
        enabled: args.config.memory?.enabled !== false,
        sessionId: args.sessionId,
        projectRoot,
    });
}

/** Binds one configuration so consumers resolve clients by session and project root alone. */
export function kernelClientResolver(config: KernelClientConfig): KernelClientResolver {
    return ({ sessionId, projectRoot }) => createKernelClient({ sessionId, projectRoot, config });
}

/** Disconnects every shared transport and drops the shared token caches between test cases; a test that dialed would otherwise leave its socket and route cache to the next one. commentlint: allow(JUDGE) */
export function resetKernelClientsForTest(): void {
    for (const shared of sharedByConnectionFile.values()) evictSharedState(shared);
    sharedByConnectionFile.clear();
}

/** `sharedByConnectionFile` orders live connection-file states from least to most recently resolved. */
export function sharedConnectionFilesForTest(): string[] {
    return [...sharedByConnectionFile.keys()];
}
