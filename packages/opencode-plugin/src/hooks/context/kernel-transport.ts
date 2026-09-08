/**
 * Adapts `HostModuleTransport` to `KernelTransport` and creates per-session
 * clients that share a transport and token cache per connection file.
 */

import { existsSync } from "node:fs";
import {
    ConnectionIdentityChangedError,
    KernelClient,
    type KernelClientResolver,
    type KernelTransport,
    type KernelTransportCall,
    StoreLifecycleError,
    type StoreLifecycleReason,
    TokenCache,
    type TokenStore,
} from "../../shared/kernel-client";
import { isRecord } from "../../shared/record-type-guard";
import { HostModuleTransport, isModuleTransportGenerationChangedResult } from "./module-transport";
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

/** Both the lifecycle manager's `storage_*` demand-start codes and the daemon's terminal `store_unavailable` application error (a storage-dependent request while the store is still opening) name a store that is reachable but not serving. commentlint: allow(JUDGE) */
const STORE_LIFECYCLE_REASONS: Readonly<Record<string, StoreLifecycleReason>> = {
    storage_starting: "store_starting",
    storage_unavailable: "store_unavailable",
    store_unavailable: "store_unavailable",
};

function storeLifecycleReason(error: unknown): StoreLifecycleReason | undefined {
    return isRecord(error) && typeof error.code === "string"
        ? STORE_LIFECYCLE_REASONS[error.code]
        : undefined;
}

/** A demand-start-capable transport is reachable with no connection file because `call` starts its daemon; every other origin keeps the synchronous stat that answers `daemon_absent` before any dial. The daemon dispatches on the method encoded in the body, so `call` refuses a body whose `method` differs from the checked one; otherwise a permitted `args.method` could carry a non-kernel body to a destructive handler. The connection identity is the module's generation: a body built under an older one is refused before any send, the same generation is handed to the module as `expectedGeneration` so it re-checks after lane admission and route settlement, and the module's own proven-not-sent replay is disabled with `generationSensitive`; a body carrying one daemon's tokens is therefore never delivered to its successor. commentlint: allow(JUDGE) */
export function createKernelTransport(transport: HostModuleTransport): KernelTransport {
    const connectionIdentity = (): string => String(transport.generation);
    return {
        connectionFileExists: () =>
            transport.canDemandStart() || existsSync(transport.connectionFilePath),
        connectionIdentity,
        async call(args: KernelTransportCall): Promise<unknown> {
            if (!isKernelMethod(args.method)) {
                throw new Error(`kernel transport refuses non-kernel method ${args.method}`);
            }
            if (!isRecord(args.body) || args.body.method !== args.method) {
                throw new Error(
                    `kernel transport refuses a body whose encoded method is not ${args.method}`,
                );
            }
            const expectedGeneration =
                args.connectionIdentity === undefined
                    ? transport.generation
                    : Number(args.connectionIdentity);
            if (expectedGeneration !== transport.generation) {
                throw new ConnectionIdentityChangedError();
            }
            let result: unknown;
            try {
                result = await transport.call({
                    sessionId: args.sessionId,
                    projectRoot: args.projectRoot,
                    method: args.method,
                    body: args.body,
                    generationSensitive: true,
                    expectedGeneration,
                    ...(args.signal ? { signal: args.signal } : {}),
                    ...(args.timeoutMs === undefined ? {} : { timeoutMs: args.timeoutMs }),
                });
            } catch (error) {
                const reason = storeLifecycleReason(error);
                if (reason !== undefined) throw new StoreLifecycleError(reason);
                throw error;
            }
            if (isModuleTransportGenerationChangedResult(result)) {
                throw new ConnectionIdentityChangedError();
            }
            return result;
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
    /** Process-unique per state, so the identity a client's view reports also changes when this state is evicted and a replacement's fresh module starts its generations from zero. commentlint: allow(JUDGE) */
    epoch: number;
    tokens: TokenCache;
    /** The `module.generation` the tokens were minted under; a later generation means the connection was invalidated, so the daemon behind the connection file may differ and the tokens are discarded. commentlint: allow(JUDGE) */
    tokenGeneration: number;
    /** Calls that have selected this state's adapter and not yet settled. A state with active calls is never evicted: it must stay the live state for its connection file until they settle, so the tokens in their responses land in the cache of the state that served them and a session close can still reach its routes. commentlint: allow(JUDGE) */
    activeCalls: number;
    /** What clients hold: views that resolve to the live state for this connection file on every use, so a client that outlives this state's eviction follows the map to its replacement — its transport never redials the evicted `module`, and its tokens are never carried from one daemon to the next. commentlint: allow(JUDGE) */
    transport: KernelTransport;
    tokenStore: TokenStore;
    /** The set orders project roots from least to most recently resolved and bounds per-project token buckets. */
    tokenProjectOrder: Set<string>;
}

/** Cap on project roots whose token buckets the shared cache retains per connection file; resolving a client past the cap evicts the least-recently-resolved root's tokens. commentlint: allow(JUDGE) */
export const MAX_TOKEN_CACHE_PROJECTS = 32;

/** Cap on connection files whose shared transports the process retains: a long-lived host that `/cd`s across projects with distinct `connection_file` values would otherwise accumulate one live transport — socket, token cache, route cache — per daemon configuration forever. Resolving a client past the cap disconnects and drops the least-recently-resolved idle state; a state with calls in flight is skipped, so the map exceeds the cap only by the number of connection files busy at that moment and is trimmed again on the next resolution. A client resolved before its state's eviction reaches the replacement state through its views on its next use. commentlint: allow(JUDGE) */
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

/** The state's tokens, emptied first if the transport's connection generation moved since they were minted. commentlint: allow(JUDGE) */
function currentTokens(shared: SharedKernelState): TokenCache {
    const generation = shared.module.generation;
    if (generation !== shared.tokenGeneration) {
        for (const projectRoot of shared.tokenProjectOrder) shared.tokens.dropProject(projectRoot);
        shared.tokenProjectOrder.clear();
        shared.tokenGeneration = generation;
    }
    return shared.tokens;
}

const sharedByConnectionFile = new Map<string, SharedKernelState>();

/** Drops and disconnects the least-recently-resolved idle states until the map is within the cap; a state with calls in flight is skipped and stays the live state for its key. commentlint: allow(JUDGE) */
function trimSharedStates(): void {
    if (sharedByConnectionFile.size <= MAX_CONNECTION_FILE_STATES) return;
    for (const [key, shared] of sharedByConnectionFile) {
        if (shared.activeCalls > 0) continue;
        sharedByConnectionFile.delete(key);
        shared.module.disconnect();
        if (sharedByConnectionFile.size <= MAX_CONNECTION_FILE_STATES) return;
    }
}

/** Tags the key so the managed default (`undefined`, may demand-start) never shares a state with an explicit empty path (`""`, never demand-starts); `resolveConnectionOrigin` distinguishes them by presence, not value. commentlint: allow(JUDGE) */
function connectionFileKey(connectionFile: string | undefined): string {
    return connectionFile === undefined ? "managed-default" : `explicit:${connectionFile}`;
}

/** The live state for a connection file, created if its previous state was evicted; a hit does not count as a resolution, so recency still tracks `createKernelClient`. commentlint: allow(JUDGE) */
function liveState(connectionFile: string | undefined): SharedKernelState {
    return (
        sharedByConnectionFile.get(connectionFileKey(connectionFile)) ?? sharedState(connectionFile)
    );
}

function viewIdentity(state: SharedKernelState): string {
    return `${state.epoch}:${state.adapter.connectionIdentity?.() ?? ""}`;
}

function liveTransport(connectionFile: string | undefined): KernelTransport {
    return {
        connectionFileExists: () => liveState(connectionFile).adapter.connectionFileExists(),
        connectionIdentity: () => viewIdentity(liveState(connectionFile)),
        async call(args) {
            // The view's identity wraps the adapter's; the adapter compares against its own, so the wrapper is checked here and stripped before delegation. commentlint: allow(JUDGE)
            const state = liveState(connectionFile);
            let delegated = args;
            if (args.connectionIdentity !== undefined) {
                const [epoch, ...rest] = args.connectionIdentity.split(":");
                if (Number(epoch) !== state.epoch) throw new ConnectionIdentityChangedError();
                delegated = { ...args, connectionIdentity: rest.join(":") };
            }
            state.activeCalls += 1;
            try {
                return await state.adapter.call(delegated);
            } finally {
                state.activeCalls -= 1;
            }
        },
        ensureRoute: (args) => liveState(connectionFile).adapter.ensureRoute(args),
    };
}

/** Every access goes through `currentTokens`, so tokens minted before a reconnect are gone before they can be read or written past. A write that names the connection identity it was served under is dropped when that identity is no longer the live one, so a response from a connection that has since been replaced cannot seed the replacement's cache. Writes touch the project first, so a bucket a retained client fills in a replacement state is tracked by `tokenProjectOrder` and stays subject to `MAX_TOKEN_CACHE_PROJECTS`. commentlint: allow(JUDGE) */
function liveTokenStore(connectionFile: string | undefined): TokenStore {
    const readable = (): TokenCache => currentTokens(liveState(connectionFile));
    const writable = (root: string, identity: string | undefined): TokenCache | undefined => {
        const state = liveState(connectionFile);
        if (identity !== undefined && identity !== viewIdentity(state)) return undefined;
        const tokens = currentTokens(state);
        touchTokenProject(state, root);
        return tokens;
    };
    return {
        remember: (root, rows, knownAsOf, identity) =>
            writable(root, identity)?.remember(root, rows, knownAsOf),
        rememberTokens: (root, tokens, knownAsOf, identity) =>
            writable(root, identity)?.rememberTokens(root, tokens, knownAsOf),
        get: (root, objectId) => readable().get(root, objectId),
        knownAsOfFor: (root) => readable().knownAsOfFor(root),
        dropProject: (root) => {
            const state = liveState(connectionFile);
            currentTokens(state).dropProject(root);
            state.tokenProjectOrder.delete(root);
        },
        size: (root) => readable().size(root),
    };
}

let nextSharedStateEpoch = 0;

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
    nextSharedStateEpoch += 1;
    shared = {
        module,
        adapter: createKernelTransport(module),
        epoch: nextSharedStateEpoch,
        tokens: new TokenCache(),
        tokenGeneration: module.generation,
        activeCalls: 0,
        transport: liveTransport(connectionFile),
        tokenStore: liveTokenStore(connectionFile),
        tokenProjectOrder: new Set(),
    };
    sharedByConnectionFile.set(key, shared);
    trimSharedStates();
    return shared;
}

/** A disabled client's `gate` answers `disabled` before any transport use, so it must not occupy a shared-state slot and evict a live transport. commentlint: allow(JUDGE) */
const DISABLED_TRANSPORT: KernelTransport = {
    connectionFileExists: () => false,
    call: () => Promise.reject(new Error("kernel transport unavailable: memory is disabled")),
    ensureRoute: async () => {},
};

interface KernelClientIdentity {
    sessionId: string;
    projectRoot: string;
    config: KernelClientConfig;
}

/** A client either shares the process-wide transport and token cache for its connection file, or brings its own transport. Explicit `tokens` are only accepted with an explicit `transport`: a token's `known_as_of` is a position in one daemon's event sequence, and the shared transport's cache is the one that follows that transport's reconnects and evictions, so a caller-owned cache on the shared transport would outlive the daemon its tokens came from. Without `tokens`, a custom transport gets a fresh cache per client; pass `tokens` to keep mutation-token continuity across clients on the same custom transport. commentlint: allow(JUDGE) */
export type CreateKernelClientArgs =
    | (KernelClientIdentity & { transport?: undefined; tokens?: undefined })
    | (KernelClientIdentity & { transport: KernelTransport; tokens?: TokenCache });

/** Applies `memory.enabled` to every client. Enabled clients for the same connection file share a transport (one dial, one route cache) and a token cache (tokens are keyed by project, not session), and take the root the transport canonicalizes, so a symlinked and a resolved spelling of one project derive the same operation keys and token bucket as the route they are bound to. commentlint: allow(JUDGE) */
export function createKernelClient(args: CreateKernelClientArgs): KernelClient {
    const enabled = args.config.memory?.enabled !== false;
    const shared =
        args.transport || !enabled ? null : sharedState(args.config.subc?.connection_file);
    const projectRoot = shared ? shared.module.canonicalRoot(args.projectRoot) : args.projectRoot;
    if (shared) {
        currentTokens(shared);
        touchTokenProject(shared, projectRoot);
    }
    return new KernelClient({
        transport: args.transport ?? shared?.transport ?? DISABLED_TRANSPORT,
        tokens: args.tokens ?? shared?.tokenStore ?? new TokenCache(),
        enabled,
        sessionId: args.sessionId,
        projectRoot,
    });
}

/** Binds one configuration so consumers resolve clients by session and project root alone. */
export function kernelClientResolver(config: KernelClientConfig): KernelClientResolver {
    return ({ sessionId, projectRoot }) => createKernelClient({ sessionId, projectRoot, config });
}

/** Releases every route the shared transport for `config` holds for `sessionId`. Each session's first call opens a host route per project root, and the host's route capacity is finite and shared across connections, so a long-lived process that serves many sessions must release them at session end rather than at connection teardown. A connection file with no live shared state has no routes to release. commentlint: allow(JUDGE) */
export function closeKernelSession(config: KernelClientConfig, sessionId: string): void {
    sharedByConnectionFile
        .get(connectionFileKey(config.subc?.connection_file))
        ?.module.closeSession(sessionId);
}

/** Disconnects every shared transport and drops the shared token caches between test cases; a test that dialed would otherwise leave its socket and route cache to the next one. commentlint: allow(JUDGE) */
export function resetKernelClientsForTest(): void {
    for (const shared of sharedByConnectionFile.values()) shared.module.disconnect();
    sharedByConnectionFile.clear();
}

/** `sharedByConnectionFile` orders live connection-file states from least to most recently resolved. */
export function sharedConnectionFilesForTest(): string[] {
    return [...sharedByConnectionFile.keys()];
}

/** Returns live shared state for `config`, or `undefined` when no enabled client has resolved it. */
export function sharedStateForTest(
    config: KernelClientConfig,
): Pick<SharedKernelState, "module" | "transport"> | undefined {
    const shared = sharedByConnectionFile.get(connectionFileKey(config.subc?.connection_file));
    return shared ? { module: shared.module, transport: shared.transport } : undefined;
}
