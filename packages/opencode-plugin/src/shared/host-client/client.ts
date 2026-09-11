/**
 * `HostClient` is a routed and managed consumer facade over the connection-generation engine.
 * engine.
 *
 * `HostClient` coalesces concurrent connection attempts into one connect operation.
 * `HostClient` rereads the connection file and fully reauthenticates after generation retirement.
 * `HostClient` caches managed routes and validates control-plane responses.
 * `HostClient` bounds and redacts diagnostics; the generation layer never imports this module.
 *
 * `request()` never replays a body.
 * `call()` owns one replay token per call.
 * `call()` spends its replay token only after proven `not_sent` or terminal `unknown_channel`, after evicting the route.
 * `call()` spends its replay token only while the caller is active and the operation deadline remains live.
 * `outcome_unknown` is never replayed by any facade path.
 */

import { access } from "node:fs/promises";
import {
    type ConnectionDiagnosticEvent,
    ConnectionGeneration,
    type ConnectionGenerationOptions,
    type JsonReceiveBody,
    type PendingRequest,
    type RequestTerminal,
    type RetirementInfo,
    type RetirementReason,
} from "./connection";
import {
    ConnectionFileError,
    type ConnectionSnapshot,
    readConnectionFile,
} from "./connection-file";
import { credentialFingerprints } from "./credential-fingerprint";
import { armExpiryTimer, Deadline, defaultMonotonicClock, type MonotonicClock } from "./deadline";
import {
    DAEMON_GENERATION_CHANGED_CODE,
    HostCallError,
    HostClientError,
    isHostCallError,
    SocketTimeoutError,
} from "./errors";
import { bytesFrameBody, type DirectFrameBody, ReceiveLease, utf8FrameBody } from "./frame-channel";
import {
    belongsToConnection,
    createRouteHandle,
    newConnectionToken,
    type RouteHandle,
    StaleRouteHandleError,
} from "./route-handle";
import { serializedJsonText } from "./serialized-json-body";
import type {
    AuthenticatedPeer,
    BindIdentity,
    CatalogEntry,
    CatalogSnapshot,
    ConnectOptions,
    ConsumerIdentity,
    HostStatusSnapshot,
    ManagedCallOptions,
    ManagedRouteKind,
    PublicationDiagnostics,
    RequestOptions,
    RouteTarget,
} from "./types";
import { AdmissionClass, sameDaemonId } from "./types";

/** Preserves the repo's current 2-second TypeScript handshake budget. */
const DEFAULT_HANDSHAKE_TIMEOUT_MS = 2_000;
/* */
const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
/** `call()` shares one route-open deadline across the managed retry loop. */
const DEFAULT_ROUTE_OPEN_DEADLINE_MS = 30_000;
/** `shutdown()` uses a separate deadline for route and connection Goodbye. */
const DEFAULT_SHUTDOWN_DEADLINE_MS = 5_000;
/** Channel-0 control bodies are capped below the frame limit (wire doc 7.1). */
const MAX_CONTROL_BODY_LEN = 65_536;
/**
 * `SETUP_RETRY_*` governs allowlisted `route.open` retries and stale-success replacement pacing in both setup loops.
 */
const SETUP_RETRY_BASE_MS = 100;
const SETUP_RETRY_CAP_MS = 2_000;
const DEFAULT_MAX_DIAGNOSTIC_EVENTS_PER_SECOND = 500;
const MAX_DIAGNOSTIC_STRING_LEN = 128;

export const EIDNARA_MODULE_ID_ENV = "EIDNARA_MODULE_ID";
export const EIDNARA_LAUNCH_NONCE_ENV = "EIDNARA_LAUNCH_NONCE";

const DEFAULT_MANAGED_TARGET_KIND: ManagedRouteKind = "management_surface";

/**
 * A diagnostics event contains only redacted frame identity, byte counts, and connection metadata.
 * Diagnostics events exclude keys, proofs, nonces, body bytes, and full bind identities.
 */
export interface HostDiagnosticsEvent {
    readonly type: ConnectionDiagnosticEvent["type"] | "connected" | "parse" | "retired";
    /** `timestampMs` records wall-clock milliseconds at emission. */
    readonly atMs: number;
    readonly frameType?: number;
    readonly channel?: number;
    readonly epoch?: number;
    readonly corr?: bigint;
    readonly len?: number;
    readonly daemonVer?: string;
    readonly pid?: number;
    readonly reason?: string;
    readonly transport?: "shm";
}

export type HostDiagnosticsObserver = (event: HostDiagnosticsEvent) => void;

/**
 * Channel-0 control operations take the same daemon fence as routed requests: `host.shutdown` stops whichever host
 * the client is connected to, and reconnect after retirement can bind a successor incarnation the caller never validated.
 */
export type ControlCallOptions = Pick<RequestOptions, "timeoutMs" | "expectedDaemonId">;

/**
 * `ConnectOptions` defines consumer-facing construction options; remaining options bound policy or inject dependencies.
 */
export interface HostClientOptions extends ConnectOptions {
    /** The clock supplies monotonic time for every operation deadline. */
    clock?: MonotonicClock;
    /** The sleep function injects backoff delays so retries are deterministic in tests. */
    sleep?: (ms: number) => Promise<void>;
    requestTimeoutMs?: number;
    routeOpenDeadlineMs?: number;
    shutdownDeadlineMs?: number;
    /**
     * The `afterOpen` hook is forwarded to connection-file reads so tests can race snapshots against deadlines deterministically.
     */
    connectionFileAfterOpen?: () => void | Promise<void>;
    /**
     * The diagnostics observer receives frozen, size- and rate-bounded redacted events; observer exceptions are swallowed.
     * excess events are dropped rather than blocking protocol work.
     */
    diagnostics?: HostDiagnosticsObserver;
    maxDiagnosticEventsPerSecond?: number;
    /** @internal Test-only complete-frame channel seam forwarded to every `ConnectionGeneration`. */
    channelFactory?: ConnectionGenerationOptions["channelFactory"];
}

interface ActiveConnection {
    readonly generation: ConnectionGeneration;
    /** `connectionToken` binds this connection's route handles. */
    readonly token: object;
    readonly snapshot: ConnectionSnapshot;
    readonly liveRoutes: Map<number, RouteHandle>;
    /**
     * `earlyRouteGoodbyes` records route Goodbyes received after `route.open` responds but before its caller installs
     * the route handle; one drain can deliver both frames before the opener's continuation runs.
     */
    readonly earlyRouteGoodbyes: Map<number, number>;
}

interface CachedManagedRoute {
    readonly target: Extract<RouteTarget, { kind: ManagedRouteKind }>;
    /**
     * Every `route.open` derives `credential_fingerprints` from this caller-supplied identity under the current connection key.
     * Deriving from a previous derivation would carry a fingerprint the current credential row no longer produces.
     */
    readonly identity: BindIdentity;
    /** The identity the `route.open` that produced `handle` carried; null until a route is bound. */
    boundIdentity: BindIdentity | null;
    readonly consumerIdentity: ConsumerIdentity | undefined;
    handle: RouteHandle | null;
    opening: SetupFlight<RouteHandle> | null;
    /**
     * `closeRoute` marks an in-flight open as closed; the open must not install its handle and instead sends best-effort route Goodbye.
     */
    closed: boolean;
}

/**
 * `SetupFlight` shares a connect or managed route open and records explicit replacement eligibility.
 * `SetupFlight`'s creator awaits `promise` directly; each joiner races it against its own stage deadline.
 * `replaceable` becomes true only at owner-budget-exhaustion exits and at an owner's daemon-fence rejection, so a surviving joiner may coalesce one replacement; permanent failures and close outcomes leave it false.
 */
interface SetupFlight<T> {
    promise: Promise<T>;
    replaceable: boolean;
}

/**
 * `raceAgainstStage` rejects only after `stage.isExpired()`; `armExpiryTimer` re-arms until expiry and rejects post-expiry fulfillment.
 * `flight`'s creation-time rejection observer prevents unhandled rejections when callers abandon the flight after losing the race.
 */
async function raceAgainstStage<T>(
    flight: Promise<T>,
    stage: Deadline,
    makeError: () => Error,
): Promise<T> {
    let cancelTimer: (() => void) | undefined;
    try {
        const result = await Promise.race([
            flight,
            new Promise<never>((_resolve, reject) => {
                cancelTimer = armExpiryTimer(stage, () => reject(makeError()));
            }),
        ]);
        // `raceAgainstStage` rejects setup that settles after the caller's stage expires, even when its settlement callback runs before the expiry timer; it does not attribute the shared flight's failure to that caller.
        if (stage.isExpired()) throw makeError();
        return result;
    } catch (error) {
        if (stage.isExpired()) throw makeError();
        throw error;
    } finally {
        cancelTimer?.();
    }
}

function connectionStageError(): SocketTimeoutError {
    return new SocketTimeoutError(
        "connection setup stage expired before the shared connect completed",
    );
}

/**
 * `settleWithinDeadline` returns once `flight` settles or `deadline` passes, whichever is first, and discards the
 * flight's outcome: teardown only needs to know that setup is no longer running.
 */
async function settleWithinDeadline(flight: Promise<unknown>, deadline: Deadline): Promise<void> {
    let cancelWait: (() => void) | undefined;
    const wait = new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, deadline.remainingMs());
        cancelWait = () => {
            clearTimeout(timer);
            resolve();
        };
    });
    try {
        await Promise.race([
            flight.then(
                () => undefined,
                () => undefined,
            ),
            wait,
        ]);
    } finally {
        cancelWait?.();
    }
}

function routeStageError(): HostCallError {
    return new HostCallError(
        "not_sent",
        "route.open deadline expired before a route was opened",
        "deadline_expired",
    );
}

function routeAbortError(): HostCallError {
    const error = new HostCallError(
        "not_sent",
        "request aborted before a route was opened",
        "aborted",
    );
    error.cleanup = Promise.resolve();
    return error;
}

/** `raceAgainstAbort` rejects for an aborted caller without cancelling the shared `flight`. */
async function raceAgainstAbort<T>(
    flight: Promise<T>,
    signal: AbortSignal | undefined,
): Promise<T> {
    if (!signal) return flight;
    if (signal.aborted) throw routeAbortError();
    let onAbort: (() => void) | undefined;
    try {
        return await Promise.race([
            flight,
            new Promise<never>((_resolve, reject) => {
                onAbort = () => reject(routeAbortError());
                signal.addEventListener("abort", onAbort, { once: true });
            }),
        ]);
    } finally {
        if (onAbort) signal.removeEventListener("abort", onAbort);
    }
}

/**
 * Stale-success re-entry in a setup loop uses an escalating bounded pacer.
 * A stale success does not consume the caller's budget, so the pacer prevents socket-speed flight replacement while the daemon retires fresh setup.
 * Each wait is clamped to the caller's stage, so pacing never extends it.
 */
function makeReplacementPacer(
    stage: Deadline,
    sleep: (ms: number) => Promise<void>,
): () => Promise<void> {
    let delayMs = SETUP_RETRY_BASE_MS;
    return async () => {
        await sleep(stage.stageBudgetMs(delayMs));
        delayMs = Math.min(delayMs * 2, SETUP_RETRY_CAP_MS);
    };
}

/**
 * `clearSlot` receives the settling flight's identity, so an old flight cannot clear a newer flight.
 * writes `replaceable`.
 */
function makeSetupFlight<T>(
    run: (flight: SetupFlight<T>) => Promise<T>,
    clearSlot: (flight: SetupFlight<T>) => void,
): SetupFlight<T> {
    const flight = { replaceable: false } as SetupFlight<T>;
    flight.promise = run(flight).finally(() => clearSlot(flight));
    flight.promise.catch(() => {});
    return flight;
}

interface RequestParams {
    channel: number;
    epoch: number;
    body: Uint8Array | DirectFrameBody;
    deadline: Deadline;
    options: RequestOptions;
    responseMode?: "json" | "binary";
    mode?: "unary" | "stream";
    /** The ceiling limits retained items for each stream-mode request. */
    maxStreamItems?: number;
    binary?: boolean;
}

function errorCode(error: unknown): string | undefined {
    if (typeof error === "object" && error !== null && "code" in error) {
        const code = (error as { code?: unknown }).code;
        if (typeof code === "string") return code;
    }
    return undefined;
}

function causeMessage(cause: unknown): string {
    if (cause === undefined) return "";
    return `: ${cause instanceof Error ? cause.message : String(cause)}`;
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
    return (
        typeof value === "object" &&
        value !== null &&
        typeof (value as { then?: unknown }).then === "function"
    );
}

/**
 * These `route.open` rejection codes indicate transient target unavailability, so a later `route.open` may succeed.
 * `route_gone` is the client's own classification of a route the host closed before its opener resumed.
 */
export function isRetryableRouteOpenCode(code: string | undefined): boolean {
    return (
        code === "unknown_module" ||
        code === "module_reloading" ||
        code === "target_unavailable" ||
        code === "module_timeout" ||
        code === "route_gone"
    );
}

/* */
export async function connectionFileExists(path: string): Promise<boolean> {
    try {
        await access(path);
        return true;
    } catch {
        return false;
    }
}

/**
 * Recognition works cross-bundle by error `name` and `kind`/`code` shape, not only `instanceof`.
 */
export function isConsumerReconnectTransient(err: unknown): boolean {
    if (err instanceof HostCallError) {
        return err.kind === "not_sent" || err.kind === "outcome_unknown";
    }
    const name = err instanceof Error ? err.name : undefined;
    if (name === "SocketClosedError" || name === "SocketTimeoutError") {
        return true;
    }
    if (name === "HostCallError") {
        const kind = (err as { kind?: unknown }).kind;
        return kind === "not_sent" || kind === "outcome_unknown";
    }
    if (name === "HostClientError" || name === "ConnectionFileError") return false;
    const code = errorCode(err);
    return (
        code === "ECONNREFUSED" ||
        code === "ECONNRESET" ||
        code === "EPIPE" ||
        code === "ETIMEDOUT" ||
        code === "ENOENT"
    );
}

/**
 * The connect-time superset of `isConsumerReconnectTransient`: a
 * `ConnectionFileError` is transient only with code `deadline_expired`; every
 * other connection-file code is terminal. Recognition works cross-bundle by
 * error `name`.
 */
export function isConnectTransient(err: unknown): boolean {
    if (isConsumerReconnectTransient(err)) return true;
    const name = err instanceof Error ? err.name : undefined;
    return name === "ConnectionFileError" && errorCode(err) === "deadline_expired";
}

/**
 * The consumer-facing client: connect, route open, raw request, managed
 * call, catalog, and bounded close over one active connection generation.
 */
export class HostClient {
    private readonly connectionFile: string;
    private readonly handshakeTimeoutMs: number;
    private readonly requestTimeoutMs: number;
    private readonly routeOpenDeadlineMs: number;
    private readonly shutdownDeadlineMs: number;
    private readonly defaultIdentity: BindIdentity | undefined;
    private readonly defaultTargetKind: ManagedRouteKind;
    private readonly credentialSource: Record<string, string | undefined> | undefined;
    private readonly clock: MonotonicClock;
    private readonly sleep: (ms: number) => Promise<void>;
    private readonly connectionFileAfterOpen: (() => void | Promise<void>) | undefined;
    private readonly diagnostics: HostDiagnosticsObserver | undefined;
    private readonly maxDiagnosticEventsPerSecond: number;
    private readonly channelFactory: ConnectionGenerationOptions["channelFactory"];

    private active: ActiveConnection | null = null;
    private connecting: SetupFlight<ActiveConnection> | null = null;
    private readonly routes = new Map<string, CachedManagedRoute>();
    /** Owner close bounds draining in-flight `route.open` attempts. */
    private readonly pendingRouteOpens = new Set<Promise<void>>();
    private closeStarted = false;
    private closePromise: Promise<void> | null = null;

    private diagWindowStartMs = 0;
    private diagWindowCount = 0;

    private constructor(options: HostClientOptions) {
        this.connectionFile = options.connectionFile;
        this.handshakeTimeoutMs = options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS;
        this.requestTimeoutMs = options.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
        this.routeOpenDeadlineMs = options.routeOpenDeadlineMs ?? DEFAULT_ROUTE_OPEN_DEADLINE_MS;
        this.shutdownDeadlineMs = options.shutdownDeadlineMs ?? DEFAULT_SHUTDOWN_DEADLINE_MS;
        this.defaultIdentity = options.identity;
        this.defaultTargetKind = options.targetKind ?? DEFAULT_MANAGED_TARGET_KIND;
        this.credentialSource = options.credentialSource;
        this.clock = options.clock ?? defaultMonotonicClock;
        this.sleep =
            options.sleep ?? ((ms: number) => new Promise((resolve) => setTimeout(resolve, ms)));
        this.connectionFileAfterOpen = options.connectionFileAfterOpen;
        this.diagnostics = options.diagnostics;
        this.maxDiagnosticEventsPerSecond =
            options.maxDiagnosticEventsPerSecond ?? DEFAULT_MAX_DIAGNOSTIC_EVENTS_PER_SECOND;
        this.channelFactory = options.channelFactory;
    }

    /**
     * Connection setup shares one handshake deadline across file reading, dialing, and authentication.
     */
    static async connect(options: HostClientOptions): Promise<HostClient> {
        const client = new HostClient(options);
        await client.ensureConnection(Deadline.start(client.handshakeTimeoutMs, client.clock));
        return client;
    }

    /* */
    get daemonVer(): string | null {
        return this.active?.snapshot.daemonVer ?? null;
    }

    /**
     * Lifecycle policy must use the retained peer identity, not {@link publication}, for compatibility and fencing.
     */
    get authenticated(): AuthenticatedPeer | null {
        const active = this.active;
        if (!active || active.generation.isRetired()) return null;
        const daemonVer = active.generation.daemonVer;
        const daemonId = active.generation.authenticatedDaemonId;
        if (daemonVer === null || daemonId === null) return null;
        return {
            daemonVer,
            // Callers must not mutate the retained identity used for fencing.
            daemonId: daemonId.slice(),
            proof: "current",
        };
    }

    /**
     * Publication metadata is untrusted display metadata and must never authorize compatibility, shutdown, or cleanup.
     */
    get publication(): PublicationDiagnostics | null {
        const active = this.active;
        if (!active) return null;
        return { daemonVer: active.snapshot.daemonVer, pid: active.snapshot.pid };
    }

    /** True after irreversible owner close begins. */
    get isClosed(): boolean {
        return this.closeStarted;
    }

    /** @internal Test-only seam; slots with no live handle and no in-flight open must not be counted here. */
    get cachedManagedRouteCount(): number {
        return this.routes.size;
    }

    /**
     * routeOpen makes one attempt under one bounded deadline and returns a connection-bound immutable handle.
     * Retry policy belongs to callers; managed call() owns an allowlisted retry loop.
     * `credentialSource` fixes the environment used to derive credential fingerprints; absent, the
     * client's own source is read at bind time.
     */
    async routeOpen(
        target: RouteTarget,
        identity: BindIdentity,
        options: Pick<RequestOptions, "expectedDaemonId"> & {
            credentialSource?: Record<string, string | undefined>;
        } = {},
    ): Promise<RouteHandle> {
        const deadline = Deadline.start(this.routeOpenDeadlineMs, this.clock);
        const active = await this.ensureConnection(deadline, options.expectedDaemonId);
        return this.controlRouteOpen(
            active,
            target,
            this.identityForConnection(active, identity, options.credentialSource),
            this.envConsumerIdentity(),
            deadline,
        );
    }

    /**
     * request sends one routed request on the supplied route generation and never replays the body.
     */
    async request(
        handle: RouteHandle,
        body: unknown,
        options: RequestOptions = {},
    ): Promise<unknown> {
        const active = this.requireLiveHandle(handle);
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const terminal = await this.awaitRequest(active.generation, {
            channel: handle.channel,
            epoch: handle.epoch,
            body: encodeBody(body),
            deadline,
            options,
        });
        return parseResponseJson(terminal);
    }

    /** Caller releases the returned ReceiveLease. */
    async requestBinary(
        handle: RouteHandle,
        body: Uint8Array,
        options: RequestOptions = {},
    ): Promise<ReceiveLease> {
        const active = this.requireLiveHandle(handle);
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const terminal = await this.awaitRequest(active.generation, {
            channel: handle.channel,
            epoch: handle.epoch,
            body: bytesFrameBody(body),
            deadline,
            options,
            responseMode: "binary",
            binary: true,
        });
        if (terminal.kind !== "response" || !(terminal.body instanceof ReceiveLease)) {
            throw new HostCallError(
                "terminal",
                "binary request did not receive a binary response",
                "expected_binary_response",
            );
        }
        return terminal.body;
    }

    /**
     * The stream is bounded by the connection pending-byte budget and a retained-item ceiling, preventing unbounded per-item decode overhead under the byte budget alone.
     */
    async requestStream<Item = unknown>(
        handle: RouteHandle,
        body: unknown,
        options: RequestOptions & { maxStreamItems?: number } = {},
    ): Promise<Item[]> {
        const active = this.requireLiveHandle(handle);
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const terminal = await this.awaitRequest(active.generation, {
            channel: handle.channel,
            epoch: handle.epoch,
            body: encodeBody(body),
            deadline,
            options,
            mode: "stream",
            ...(options.maxStreamItems === undefined
                ? {}
                : { maxStreamItems: options.maxStreamItems }),
        });
        if (terminal.kind !== "stream_end") {
            throw new HostCallError(
                "terminal",
                "stream request did not receive StreamEnd",
                "expected_stream_response",
            );
        }
        return terminal.stream.map((item) => {
            const json = requireJsonReceiveBody(item);
            if (!json.valid) {
                throw new HostCallError(
                    "terminal",
                    "stream item body was not valid JSON",
                    "invalid_response_body",
                );
            }
            return json.value as Item;
        });
    }

    /**
     * call() caches routes by target kind, module ID, identity, and consumer identity and reopens them after retirement.
     * call() replays a request at most once after an unknown-channel or not-sent failure while the caller remains active and before the deadline.
     */
    async call<Response = unknown>(
        moduleId: string,
        method: string,
        params?: unknown,
        options: ManagedCallOptions = {},
    ): Promise<Response> {
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const body = utf8FrameBody(
            JSON.stringify(params === undefined ? { method } : { method, params }),
        );
        let replaySpent = false;
        for (;;) {
            const handle = await this.managedRouteHandle(moduleId, options, deadline);
            try {
                const active = this.requireLiveHandle(handle);
                const terminal = await this.awaitRequest(active.generation, {
                    channel: handle.channel,
                    epoch: handle.epoch,
                    body,
                    deadline,
                    options,
                });
                return parseResponseJson<Response>(terminal);
            } catch (error) {
                const err = toManagedCallError(error);
                const callerActive = !this.closeStarted && options.signal?.aborted !== true;
                const mayReplay = !replaySpent && callerActive && !deadline.isExpired();
                if (err.kind === "terminal" && err.code === "unknown_channel" && mayReplay) {
                    // The host proved no dispatch; evict the dead route.
                    replaySpent = true;
                    this.evictHandle(handle);
                    continue;
                }
                if (err.kind === "not_sent" && mayReplay) {
                    replaySpent = true;
                    continue;
                }
                throw err;
            }
        }
    }

    /* */
    async catalogList(options: ControlCallOptions = {}): Promise<CatalogEntry[]> {
        return (await this.catalogSnapshot(options)).modules;
    }

    /**
     * catalog.list requires a tagged generation, closed-shape host host_ops, and per-module id, version, roles, and control_ops.
     * Any duplicate, missing field, or out-of-bounds value throws malformed_control_response; the response is never cast.
     *
     * Unknown fields are ignored, not rejected.
     * The parser ignores unknown fields so newer daemons can add fields without stranding older clients.
     * The negotiation family permits fields that closed-shape responses reject.
     *
     * timeoutMs overrides the client-wide request timeout so callers can spend only their remaining aggregate-deadline budget.
     */
    async catalogSnapshot(options: ControlCallOptions = {}): Promise<CatalogSnapshot> {
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const active = await this.ensureConnection(deadline, options.expectedDaemonId);
        const bodyText = JSON.stringify({ op: "catalog.list" });
        const parsed = await this.controlRequest(
            active,
            bodyText,
            "catalog.list",
            deadline,
            options,
        );
        return parseCatalogResponse(parsed);
    }

    /**
     * host.shutdown resolves only after its correlated success response is fully received.
     * host.shutdown resolves only after its correlated success response is fully received—the caller-observable stop-commit point.
     * `close()` and `closeAsync()` never call `host.shutdown`; they only tear down the connection.
     * `close()` and `closeAsync()` perform connection teardown only.
     */
    async hostShutdown(options: ControlCallOptions = {}): Promise<void> {
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const active = await this.ensureConnection(deadline, options.expectedDaemonId);
        const bodyText = JSON.stringify({ op: "host.shutdown" });
        await this.controlRequest(active, bodyText, "host.shutdown", deadline, options);
    }

    /** The readiness operation reads host-owned component readiness without opening a routed module. */
    async hostStatus(options: ControlCallOptions = {}): Promise<HostStatusSnapshot> {
        const deadline = Deadline.start(options.timeoutMs ?? this.requestTimeoutMs, this.clock);
        const active = await this.ensureConnection(deadline, options.expectedDaemonId);
        const bodyText = JSON.stringify({ op: "host.status" });
        const parsed = await this.controlRequest(
            active,
            bodyText,
            "host.status",
            deadline,
            options,
        );
        return parseHostStatusResponse(parsed);
    }

    /**
     * Route teardown removes exactly the supplied route generation.
     * Route teardown evicts caches, sends route Goodbye, and awaits the write within the shutdown deadline.
     */
    async closeRoute(handle: RouteHandle): Promise<void> {
        const conn = this.requireLiveHandle(handle);
        conn.liveRoutes.delete(handle.channel);
        for (const [key, cached] of this.detachCachedHandle(handle)) {
            cached.closed = true;
            this.routes.delete(key);
        }
        conn.generation.enqueueRouteGoodbye(handle.channel, handle.epoch);
        await conn.generation.flushWrites(Deadline.start(this.shutdownDeadlineMs, this.clock));
    }

    /**
     * close() starts bounded asynchronous teardown and returns immediately.
     * `closeAsync()`.
     */
    close(): void {
        void this.closeAsync();
    }

    /**
     * closeAsync() uses one bounded shutdown deadline.
     * closeAsync() drains in-flight route.open attempts.
     * A late route.open success sends route Goodbye instead of entering the route cache.
     * closeAsync() sends connection Goodbye best-effort, flushes, retires the generation, and is idempotent.
     */
    closeAsync(): Promise<void> {
        if (this.closePromise) return this.closePromise;
        this.closeStarted = true;
        this.closePromise = this.runClose();
        return this.closePromise;
    }

    // ------------------------------------------------------------------
    // The connection owner replaces a retired generation.
    // ------------------------------------------------------------------

    private async ensureConnection(
        deadline: Deadline,
        expectedDaemonId?: Uint8Array,
    ): Promise<ActiveConnection> {
        // Each caller derives one immutable handshake stage from its operation deadline and retains it through every join and replacement.
        const stage = deadline.stage(this.handshakeTimeoutMs);
        const pace = makeReplacementPacer(stage, this.sleep);
        for (;;) {
            if (this.closeStarted) throw new HostClientError("client closed", "client_closed");
            const active = this.active;
            if (active && !active.generation.isRetired()) {
                this.assertExpectedDaemon(active.generation, expectedDaemonId);
                return active;
            }
            let flight = this.connecting;
            let owner = false;
            if (!flight) {
                owner = true;
                flight = makeSetupFlight(
                    (f) => this.openConnection(stage, f),
                    (f) => {
                        if (this.connecting === f) this.connecting = null;
                    },
                );
                this.connecting = flight;
            }
            let conn: ActiveConnection;
            try {
                // The owner awaits its bounded operation directly so timeout and retirement errors propagate unchanged.
                // Each joiner waits on the shared flight against its own stage.
                conn = owner
                    ? await flight.promise
                    : await raceAgainstStage(flight.promise, stage, connectionStageError);
            } catch (error) {
                // A joiner whose stage expires detaches without mutating the shared flight.
                // Only owner-budget exhaustion of a joined flight authorizes one coalesced replacement.
                if (owner || !flight.replaceable || stage.isExpired() || this.closeStarted) {
                    throw error;
                }
                continue;
            }
            // The connection owner adopts only a still-current, non-retired generation; a stale success re-enters recovery under the unchanged stage.
            if (this.active === conn && !conn.generation.isRetired()) {
                this.assertExpectedDaemon(conn.generation, expectedDaemonId);
                return conn;
            }
            if (stage.isExpired()) throw connectionStageError();
            // The loop head adopts a live candidate without new I/O; otherwise, pace the replacement dial.
            const candidate = this.active;
            if (!candidate || candidate.generation.isRetired()) {
                await pace();
                if (stage.isExpired()) throw connectionStageError();
            }
        }
    }

    private async openConnection(
        stage: Deadline,
        flight: SetupFlight<ActiveConnection>,
    ): Promise<ActiveConnection> {
        // Reconnect rereads the file and reauthenticates; credentials are not cached across generations.
        let snapshot: ConnectionSnapshot;
        try {
            snapshot = await readConnectionFile(this.connectionFile, {
                deadline: stage,
                afterOpen: this.connectionFileAfterOpen,
            });
        } catch (error) {
            // Only `ConnectionFileError` with code `deadline_expired` makes `flight` replaceable.
            if (error instanceof ConnectionFileError && error.code === "deadline_expired") {
                flight.replaceable = true;
            }
            throw error;
        }
        let conn: ActiveConnection | null = null;
        let retiredReason: RetirementReason | null = null;
        const generation = new ConnectionGeneration({
            setupSocket: snapshot.setupSocket,
            credentials: {
                key: snapshot.key,
                daemonId: snapshot.daemonId,
                daemonVer: snapshot.daemonVer,
            },
            onRetired: (info) => {
                retiredReason = info.reason;
                if (conn) this.onGenerationRetired(conn, info);
            },
            onRouteGoodbye: (channel, epoch) => {
                if (conn) this.onRouteGoodbye(conn, channel, epoch);
            },
            // Skip per-frame event allocation entirely when no observer is
            // configured; the generation's hook check short-circuits on
            // undefined.
            onDiagnostic: this.diagnostics ? (event) => this.emitDiagnostics(event) : undefined,
            channelFactory: this.channelFactory,
        });
        conn = {
            generation,
            token: newConnectionToken(),
            snapshot,
            liveRoutes: new Map(),
            earlyRouteGoodbyes: new Map(),
        };
        try {
            await generation.start(stage);
        } catch (error) {
            // Only retirement with reason `setup_deadline` makes `flight` replaceable; auth, socket, and protocol failures do not.
            // replacement.
            if (retiredReason === "setup_deadline") flight.replaceable = true;
            throw error;
        }
        if (this.closeStarted) {
            generation.retire("owner_close");
            throw new HostClientError("client closed", "client_closed");
        }
        // A generation that retires during setup is never published.
        if (generation.isRetired()) return conn;
        this.active = conn;
        this.emitConnected(conn);
        return conn;
    }

    private onGenerationRetired(conn: ActiveConnection, info: RetirementInfo): void {
        if (this.active === conn) {
            this.active = null;
            for (const [key, cached] of this.routes) {
                this.releaseSlot(key, cached);
            }
        }
        this.emitDiagnostics({ type: "retired", reason: info.reason });
    }

    private onRouteGoodbye(conn: ActiveConnection, channel: number, epoch: number): void {
        if (this.active !== conn) return;
        const handle = conn.liveRoutes.get(channel);
        if (!handle || handle.epoch !== epoch) {
            // Store an unmatched Goodbye only while a route open is pending so its opener can observe it; otherwise
            // ignore it.
            if (this.pendingRouteOpens.size > 0) conn.earlyRouteGoodbyes.set(channel, epoch);
            return;
        }
        conn.liveRoutes.delete(channel);
        this.detachCachedHandle(handle);
    }

    /** Returns the live mandatory-ring connection owning `handle`. */
    private connectionFor(handle: RouteHandle): ActiveConnection | null {
        const conn = this.active;
        return conn !== null &&
            !conn.generation.isRetired() &&
            belongsToConnection(handle, conn.token) &&
            conn.liveRoutes.get(handle.channel) === handle
            ? conn
            : null;
    }

    /**
     * Managed-route cache eligibility: only the primary may serve a cached
     * managed handle.
     */
    private isPrimaryLiveHandle(handle: RouteHandle): boolean {
        const conn = this.connectionFor(handle);
        return conn !== null && conn === this.active;
    }

    private requireLiveHandle(handle: RouteHandle): ActiveConnection {
        const conn = this.connectionFor(handle);
        if (conn === null) throw new StaleRouteHandleError(handle);
        return conn;
    }

    private assertExpectedDaemon(
        generation: ConnectionGeneration,
        expectedDaemonId?: Uint8Array,
    ): void {
        if (expectedDaemonId === undefined) return;
        if (!sameDaemonId(generation.authenticatedDaemonId, expectedDaemonId)) {
            throw new HostCallError(
                "not_sent",
                "authenticated daemon changed after lifecycle compatibility validation",
                DAEMON_GENERATION_CHANGED_CODE,
            );
        }
    }

    private evictHandle(handle: RouteHandle): void {
        const conn = this.connectionFor(handle);
        if (conn !== null) conn.liveRoutes.delete(handle.channel);
        this.detachCachedHandle(handle);
    }

    /**
     * `closeRoute` uses the returned keys to mark and evict every cached entry for `handle`.
     * Callers that do not own `handle` only detach the route.
     */
    private detachCachedHandle(handle: RouteHandle): [string, CachedManagedRoute][] {
        const detached: [string, CachedManagedRoute][] = [];
        for (const [key, cached] of this.routes) {
            if (cached.handle === handle) {
                this.releaseSlot(key, cached);
                detached.push([key, cached]);
            }
        }
        return detached;
    }

    /**
     * A slot with neither a live handle nor an in-flight open has nothing left to serve, so it leaves the cache;
     * otherwise per-session identities would accumulate one dead slot per retirement for the client's lifetime.
     * A slot with an in-flight open stays because that open still installs into it and callers compare against it.
     */
    private releaseSlot(key: string, cached: CachedManagedRoute): void {
        cached.handle = null;
        cached.boundIdentity = null;
        if (cached.opening === null && this.routes.get(key) === cached) this.routes.delete(key);
    }

    private emitConnected(conn: ActiveConnection): void {
        this.emitDiagnostics({
            type: "connected",
            daemonVer: conn.snapshot.daemonVer.slice(0, MAX_DIAGNOSTIC_STRING_LEN),
            pid: conn.snapshot.pid,
            transport: "shm",
        });
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------

    /**
     * Wire Error terminals become a `terminal` HostCallError with the canonical body's stable code.
     * A caller abort rejects with the cleanup ticket.
     * A post-write routed abort enqueues a correlation-scoped Cancel; channel 0 never receives Cancel.
     * A channel-0 caller abort retires the generation.
     */
    private async awaitRequest(
        generation: ConnectionGeneration,
        params: RequestParams,
    ): Promise<RequestTerminal> {
        // The daemon-binding gate runs before `generation.request` sends any bytes.
        this.assertExpectedDaemon(generation, params.options.expectedDaemonId);
        if (params.options.admissionClass === AdmissionClass.Sheddable) {
            // Wire doc 6.2 permits Sheddable only on Push and StreamData. Reject before encoding so the error
            // reports `not_sent`.
            throw new HostCallError(
                "not_sent",
                "Sheddable admission is illegal on Request frames",
                "invalid_admission_class",
            );
        }
        const signal = params.options.signal;
        // An already-aborted signal rejects before admission because `generation.request` synchronously publishes
        // the body; aborting afterward yields `outcome_unknown`.
        if (signal?.aborted) {
            const aborted = new HostCallError(
                "not_sent",
                "request aborted before admission",
                "aborted",
            );
            aborted.cleanup = Promise.resolve();
            throw aborted;
        }
        const pending: PendingRequest = generation.request({
            channel: params.channel,
            epoch: params.epoch,
            body: params.body,
            deadline: params.deadline,
            mode: params.mode ?? "unary",
            ...(params.maxStreamItems === undefined
                ? {}
                : { maxStreamItems: params.maxStreamItems }),
            responseMode: params.responseMode,
            binary: params.binary,
            priority: params.options.priority,
            admissionClass: params.options.admissionClass,
        });
        let cleanup: Promise<void> | null = null;
        const onAbort = (): void => {
            cleanup = pending.abort().cleanup;
        };
        signal?.addEventListener("abort", onAbort, { once: true });
        try {
            const terminal = await pending.result;
            if (terminal.kind === "error") {
                const errorBody = requireJsonReceiveBody(terminal.body);
                const failure = terminalFromErrorBody(errorBody);
                throw failure;
            }
            return terminal;
        } catch (error) {
            if (cleanup !== null && error instanceof HostCallError) {
                if (error.kind === "outcome_unknown" && params.channel !== 0) {
                    generation.enqueueCancel(params.channel, params.epoch, pending.correlation);
                }
                error.cleanup = cleanup;
            }
            throw error;
        } finally {
            signal?.removeEventListener("abort", onAbort);
        }
    }

    /* */
    private async controlRequest(
        active: ActiveConnection,
        bodyText: string,
        expectedOp: string,
        deadline: Deadline,
        options: Pick<RequestOptions, "expectedDaemonId"> = {},
    ): Promise<Record<string, unknown>> {
        const body = Buffer.from(bodyText, "utf8");
        if (body.length > MAX_CONTROL_BODY_LEN) {
            throw new HostCallError(
                "not_sent",
                `channel-0 control body of ${body.length} bytes exceeds the ${MAX_CONTROL_BODY_LEN}-byte cap`,
                "control_body_too_large",
            );
        }
        const terminal = await this.awaitRequest(active.generation, {
            channel: 0,
            epoch: 0,
            body,
            deadline,
            options,
        });
        const responseBody = requireJsonReceiveBody(terminal.body);
        const parsed = responseBody.valid ? responseBody.value : undefined;
        if (
            typeof parsed !== "object" ||
            parsed === null ||
            Array.isArray(parsed) ||
            (parsed as { op?: unknown }).op !== expectedOp
        ) {
            throw new HostCallError(
                "terminal",
                `control response was not a tagged ${expectedOp} object`,
                "malformed_control_response",
            );
        }
        this.emitDiagnostics({
            type: "parse",
            channel: 0,
            epoch: 0,
            len: responseBody.byteLength,
        });
        return parsed as Record<string, unknown>;
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------

    private controlRouteOpen(
        active: ActiveConnection,
        target: RouteTarget,
        identity: BindIdentity,
        consumerIdentity: ConsumerIdentity | undefined,
        deadline: Deadline,
    ): Promise<RouteHandle> {
        const run = this.runRouteOpen(active, target, identity, consumerIdentity, deadline);
        const tracked = run.then(
            () => undefined,
            () => undefined,
        );
        this.pendingRouteOpens.add(tracked);
        void tracked.finally(() => {
            this.pendingRouteOpens.delete(tracked);
            // With no opener left to consume them, the recorded Goodbyes are ordinary no-ops.
            if (this.pendingRouteOpens.size === 0) active.earlyRouteGoodbyes.clear();
        });
        return run;
    }

    private async runRouteOpen(
        active: ActiveConnection,
        target: RouteTarget,
        identity: BindIdentity,
        consumerIdentity: ConsumerIdentity | undefined,
        deadline: Deadline,
    ): Promise<RouteHandle> {
        const bodyText = routeOpenBody(target, identity, consumerIdentity);
        let parsed: Record<string, unknown>;
        try {
            parsed = await this.controlRequest(active, bodyText, "route.open", deadline);
        } catch (error) {
            // KTD9: an ambiguous channel-0 route.open (possible send, no
            // terminal) has no handle and Cancel is illegal on channel 0,
            // so retire the generation before reconnecting.
            if (error instanceof HostCallError && error.kind === "outcome_unknown") {
                active.generation.retire("ambiguous_route_open", error);
            }
            throw error;
        }
        const channel = parsed.route_channel;
        const epoch = parsed.route_epoch;
        let handle: RouteHandle;
        try {
            if (typeof channel !== "number" || typeof epoch !== "number") {
                throw new RangeError("route.open response carried no numeric route handle");
            }
            handle = createRouteHandle(channel, epoch, active.token);
        } catch (error) {
            // The host may have bound a route the client cannot name, so it cannot send route Goodbye for it. Retiring
            // the generation obliges the host to settle every route on it instead of stranding the binding.
            const malformed = new HostCallError(
                "terminal",
                `route.open returned a malformed route handle${causeMessage(error)}`,
                "malformed_control_response",
                error,
            );
            active.generation.retire("protocol_violation", malformed);
            throw malformed;
        }
        if (active.liveRoutes.has(handle.channel)) {
            // Wire doc 9.4 requires route cleanup before channel reuse; installing the duplicate would strand the
            // prior route without a Goodbye.
            const duplicate = new HostCallError(
                "terminal",
                `route.open returned channel ${handle.channel}, which is already live on this connection`,
                "malformed_control_response",
            );
            active.generation.retire("protocol_violation", duplicate);
            throw duplicate;
        }
        if (active.earlyRouteGoodbyes.get(handle.channel) === handle.epoch) {
            // The host closed this route before its opener resumed; the Goodbye already settled it host-side.
            active.earlyRouteGoodbyes.delete(handle.channel);
            throw new HostCallError(
                "terminal",
                "host closed the route before route.open completed",
                "route_gone",
            );
        }
        if (this.closeStarted) {
            // During an owner-close race, the client does not cache a late route and enqueues Goodbye best-effort because failed enqueue retires the generation internally.
            active.generation.enqueueRouteGoodbye(handle.channel, handle.epoch);
            throw new HostCallError(
                "not_sent",
                "route was closed before route.open completed",
                "route_closed",
            );
        }
        active.liveRoutes.set(handle.channel, handle);
        return handle;
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------

    private async managedRouteHandle(
        moduleId: string,
        options: ManagedCallOptions,
        deadline: Deadline,
    ): Promise<RouteHandle> {
        const baseIdentity = options.identity ?? this.defaultIdentity;
        if (!baseIdentity) {
            throw new HostCallError(
                "terminal",
                "managed call requires a BindIdentity in HostClient.connect({ identity }) or call(..., { identity })",
                "missing_identity",
            );
        }
        const identity = baseIdentity;
        const kind = options.targetKind ?? this.defaultTargetKind;
        const target = { kind, module_id: moduleId } as Extract<
            RouteTarget,
            { kind: ManagedRouteKind }
        >;
        const consumerIdentity = this.envConsumerIdentity();
        // The daemon-independent key ensures one logical binding owns one slot.
        // Generation retirement makes `isPrimaryLiveHandle` reject handles from the previous daemon.
        // `assertExpectedDaemon` fences publication after daemon rotation.
        // Keying by identity would strand one cache entry per daemon rotation.
        // Identity-based keys would let callers without a daemon expectation open a second route for the same target.
        const key = routeCacheKey(target, identity, consumerIdentity);
        // One immutable route-open stage per caller is derived once and kept through every join and replacement decision.
        const stage = deadline.stage(this.routeOpenDeadlineMs);
        const pace = makeReplacementPacer(stage, this.sleep);
        const signal = options.signal;
        for (;;) {
            if (signal?.aborted) throw routeAbortError();
            let cached = this.routes.get(key);
            if (!cached) {
                cached = {
                    target,
                    identity,
                    boundIdentity: null,
                    consumerIdentity,
                    handle: null,
                    opening: null,
                    closed: false,
                };
                this.routes.set(key, cached);
            }
            // Only the active generation serves cached managed handles.
            const active = cached.handle ? this.connectionFor(cached.handle) : null;
            if (cached.handle && active) {
                const currentIdentity = this.identityForConnection(active, identity);
                if (sameCredentialFingerprints(currentIdentity, cached.boundIdentity)) {
                    return cached.handle;
                }
                active.liveRoutes.delete(cached.handle.channel);
                active.generation.enqueueRouteGoodbye(cached.handle.channel, cached.handle.epoch);
                cached.handle = null;
                cached.boundIdentity = null;
            }
            let flight = cached.opening;
            let owner = false;
            if (!flight) {
                owner = true;
                const slot = cached;
                flight = makeSetupFlight(
                    (f) => this.openCachedRoute(slot, stage, f, options.expectedDaemonId),
                    (f) => {
                        if (slot.opening !== f) return;
                        slot.opening = null;
                        if (slot.handle === null) this.releaseSlot(key, slot);
                    },
                );
                cached.opening = flight;
            }
            let handle: RouteHandle;
            try {
                // The owner awaits directly; a joiner races its own stage. Either detaches when its signal aborts.
                handle = owner
                    ? await raceAgainstAbort(flight.promise, signal)
                    : await raceAgainstStage(
                          raceAgainstAbort(flight.promise, signal),
                          stage,
                          routeStageError,
                      );
            } catch (error) {
                if (owner || !flight.replaceable || stage.isExpired() || this.closeStarted) {
                    throw error;
                }
                continue;
            }
            // Before considering any body-replay token, stale-success handling adopts only the cache's current live handle for this route identity.
            if (
                this.routes.get(key) === cached &&
                cached.handle === handle &&
                this.isPrimaryLiveHandle(handle)
            ) {
                return handle;
            }
            if (stage.isExpired()) throw routeStageError();
            // The loop head adopts an installed live handle without new I/O.
            const current = this.routes.get(key);
            if (!(current?.handle && this.isPrimaryLiveHandle(current.handle))) {
                await pace();
                if (stage.isExpired()) throw routeStageError();
            }
        }
    }

    /**
     * The owner opens one cached managed route under a bounded route-open deadline.
     * Only allowlisted momentary `route.open` rejections retry with bounded backoff.
     * Transient connection failures reconnect.
     * application body is never sent before route success.
     */
    private async openCachedRoute(
        cached: CachedManagedRoute,
        deadline: Deadline,
        flight: SetupFlight<RouteHandle>,
        expectedDaemonId?: Uint8Array,
    ): Promise<RouteHandle> {
        let delayMs = SETUP_RETRY_BASE_MS;
        const backoff = async (): Promise<boolean> => {
            await this.sleep(deadline.stageBudgetMs(delayMs));
            delayMs = Math.min(delayMs * 2, SETUP_RETRY_CAP_MS);
            return !deadline.isExpired();
        };
        for (;;) {
            if (cached.closed || this.closeStarted) {
                throw new HostCallError(
                    "not_sent",
                    "route was closed before route.open completed",
                    "route_closed",
                );
            }
            if (deadline.isExpired()) {
                // The owner's route-open budget determines when route opening fails.
                flight.replaceable = true;
                throw routeStageError();
            }
            let active: ActiveConnection;
            try {
                active = await this.ensureConnection(deadline, expectedDaemonId);
            } catch (error) {
                if (error instanceof HostCallError) {
                    // The fence rejects only this owner's `expectedDaemonId`, so joiners may replace the flight.
                    if (error.code === DAEMON_GENERATION_CHANGED_CODE) flight.replaceable = true;
                    throw error;
                }
                // A stage-expired snapshot reconnects under the clamped handshake budget; other `ConnectionFileError`s are terminal.
                // A snapshot that outlives its stage uses the clamped handshake budget, not the route budget, and reconnects as a transient setup failure.
                // Every other connection-file failure is terminal.
                const transient = isConnectTransient(error);
                if (transient && !this.closeStarted) {
                    if (await backoff()) continue;
                    // Transient reconnects continue until the owner's budget expires or a connection succeeds.
                    flight.replaceable = true;
                }
                throw new HostCallError(
                    transient ? "not_sent" : "terminal",
                    `route.open could not run because connect failed${causeMessage(error)}`,
                    errorCode(error),
                    error,
                );
            }
            if (cached.handle && this.isPrimaryLiveHandle(cached.handle)) return cached.handle;
            const boundIdentity = this.identityForConnection(active, cached.identity);
            try {
                const handle = await this.controlRouteOpen(
                    active,
                    cached.target,
                    boundIdentity,
                    cached.consumerIdentity,
                    deadline,
                );
                if (cached.closed) {
                    active.liveRoutes.delete(handle.channel);
                    active.generation.enqueueRouteGoodbye(handle.channel, handle.epoch);
                    throw new HostCallError(
                        "not_sent",
                        "route was closed before route.open completed",
                        "route_closed",
                    );
                }
                cached.handle = handle;
                cached.boundIdentity = boundIdentity;
                return handle;
            } catch (error) {
                if (!isHostCallError(error)) {
                    throw new HostCallError(
                        "terminal",
                        `route.open failed for module ${cached.target.module_id}${causeMessage(error)}`,
                        errorCode(error),
                        error,
                    );
                }
                if (error.code === "route_closed" || this.closeStarted) throw error;
                if (error.kind === "terminal" && isRetryableRouteOpenCode(error.code)) {
                    if (await backoff()) continue;
                    // The allowlisted retry budget is the owner's budget.
                    flight.replaceable = true;
                    throw new HostCallError(
                        "not_sent",
                        `route.open failed for module ${cached.target.module_id}: ${error.code} (route-open retry budget exhausted)`,
                        error.code,
                        error,
                    );
                }
                if (error.kind === "not_sent" || error.kind === "outcome_unknown") {
                    // When `error.code` is `control_body_too_large`, retries and replacements cannot change the deterministic encoding failure.
                    if (error.code === "control_body_too_large") throw error;
                    // An `outcome_unknown` channel-0 `route.open` retires the generation because Cancel is illegal on channel 0 and no terminal or handle exists.
                    // next loop iteration reconnects under the same deadline.
                    if (await backoff()) continue;
                    // After the owner's route-open budget expires, remaining callers may coalesce on one replacement route.
                    flight.replaceable = true;
                    throw error;
                }
                throw error;
            }
        }
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------

    private async runClose(): Promise<void> {
        const deadline = Deadline.start(this.shutdownDeadlineMs, this.clock);
        // Both waits are bounded by the shutdown deadline, not by the handshake or route-open budgets they observe.
        if (this.connecting) {
            await settleWithinDeadline(this.connecting.promise, deadline);
        }
        if (this.pendingRouteOpens.size > 0) {
            await settleWithinDeadline(Promise.all([...this.pendingRouteOpens]), deadline);
        }
        const conns =
            this.active !== null && !this.active.generation.isRetired() ? [this.active] : [];
        try {
            for (const conn of conns) conn.generation.enqueueConnectionGoodbye();
            await Promise.all(conns.map((conn) => conn.generation.flushWrites(deadline)));
        } catch {
            // Goodbye is best-effort; an unsent Goodbye must not leave the generation live after close.
        } finally {
            for (const conn of conns) conn.generation.retire("owner_close");
        }
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------

    private emitDiagnostics(event: Omit<HostDiagnosticsEvent, "atMs">): void {
        const observer = this.diagnostics;
        if (!observer) return;
        // The window rolls on the monotonic clock: a backward wall-clock step would otherwise leave the elapsed
        // value negative and latch the limiter shut until wall time passed the frozen window start.
        const monotonicMs = this.clock();
        if (!(monotonicMs - this.diagWindowStartMs < 1_000)) {
            this.diagWindowStartMs = monotonicMs;
            this.diagWindowCount = 0;
        }
        this.diagWindowCount += 1;
        if (this.diagWindowCount > this.maxDiagnosticEventsPerSecond) return;
        try {
            // A void-typed observer may still be an `async` function; its rejection is swallowed too.
            const result: unknown = observer(Object.freeze({ ...event, atMs: Date.now() }));
            if (isThenable(result)) result.then(undefined, () => {});
        } catch {
            // Observer exceptions must never affect protocol work.
        }
    }

    private envConsumerIdentity(): ConsumerIdentity | undefined {
        const moduleId = process.env[EIDNARA_MODULE_ID_ENV];
        const launchNonce = process.env[EIDNARA_LAUNCH_NONCE_ENV];
        if (!moduleId || !launchNonce) return undefined;
        return { module_id: moduleId, launch_nonce: launchNonce };
    }

    /**
     * Managed harnesses derive `credential_fingerprints` solely from `active.snapshot.key`; the host rejects values
     * retained from a prior key, so an empty derivation removes a caller-supplied claim instead of forwarding it.
     */
    private identityForConnection(
        active: ActiveConnection,
        identity: BindIdentity,
        credentialSource: Record<string, string | undefined> | undefined = this.credentialSource,
    ): BindIdentity {
        if (
            credentialSource === undefined ||
            (identity.harness !== "opencode" && identity.harness !== "pi")
        ) {
            return identity;
        }
        const fingerprints = credentialFingerprints(
            active.snapshot.key,
            identity.harness,
            credentialSource,
        );
        const { credential_fingerprints: _supplied, ...base } = identity;
        return Object.keys(fingerprints).length === 0
            ? base
            : { ...base, credential_fingerprints: fingerprints };
    }
}

// ----------------------------------------------------------------------
// ----------------------------------------------------------------------

function encodeBody(body: unknown): DirectFrameBody {
    if (body instanceof Uint8Array) return bytesFrameBody(body);
    const text = serializedJsonText(body) ?? JSON.stringify(body);
    if (text === undefined) throw new TypeError("request body is not JSON serializable");
    return utf8FrameBody(text);
}

/* */
function routeOpenBody(
    target: RouteTarget,
    identity: BindIdentity,
    consumerIdentity: ConsumerIdentity | undefined,
): string {
    const canonicalTarget =
        target.kind === "internal_service"
            ? { kind: target.kind, module_id: target.module_id, service_id: target.service_id }
            : { kind: target.kind, module_id: target.module_id };
    const canonicalIdentity = {
        project_root: identity.project_root,
        harness: identity.harness,
        session: identity.session,
        ...(identity.credential_fingerprints === undefined
            ? {}
            : { credential_fingerprints: identity.credential_fingerprints }),
    };
    return JSON.stringify(
        consumerIdentity
            ? {
                  op: "route.open",
                  target: canonicalTarget,
                  identity: canonicalIdentity,
                  consumer_identity: {
                      module_id: consumerIdentity.module_id,
                      launch_nonce: consumerIdentity.launch_nonce,
                  },
              }
            : { op: "route.open", target: canonicalTarget, identity: canonicalIdentity },
    );
}

type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

function requireJsonReceiveBody(body: RequestTerminal["body"]): JsonReceiveBody {
    if (body instanceof ReceiveLease) {
        // A quarantined release throws only after `onRelease` accounts for the outcome.
        // The method throws `unexpected_binary_response` only after `onRelease` accounts for the quarantined outcome.
        if (!body.isReleased()) {
            try {
                body.release();
            } catch {
                // `onRelease` must run before the throw so it accounts for quarantine.
            }
        }
        throw new HostCallError(
            "terminal",
            "response body was unexpectedly binary",
            "unexpected_binary_response",
        );
    }
    return body;
}

/** `code` drives retry policy, so it is read only from a canonical body; unknown members are ignored. */
function terminalFromErrorBody(body: JsonReceiveBody): HostCallError {
    if (typeof body.value === "object" && body.value !== null && !Array.isArray(body.value)) {
        const parsed = body.value as {
            code?: unknown;
            message?: unknown;
            retry_after_ms?: unknown;
        };
        const retryAfterMs = parsed.retry_after_ms;
        const canonical =
            typeof parsed.code === "string" &&
            typeof parsed.message === "string" &&
            (retryAfterMs === undefined ||
                (typeof retryAfterMs === "number" &&
                    Number.isSafeInteger(retryAfterMs) &&
                    retryAfterMs >= 0));
        if (canonical) {
            const error = new HostCallError(
                "terminal",
                parsed.message as string,
                parsed.code as string,
            );
            if (retryAfterMs !== undefined) error.retry_after_ms = retryAfterMs as number;
            return error;
        }
        return new HostCallError(
            "terminal",
            "daemon error body was not a canonical ErrorBody",
            "malformed_error_body",
        );
    }
    return new HostCallError("terminal", body.text || "daemon error", "malformed_error_body");
}

function parseResponseJson<Response = JsonValue>(terminal: RequestTerminal): Response {
    const body = requireJsonReceiveBody(terminal.body);
    if (body.valid) return body.value as Response;
    throw new HostCallError(
        "terminal",
        "response body was not valid JSON",
        "invalid_response_body",
    );
}

const MAX_CATALOG_MODULES = 64;
const MAX_CATALOG_OPS = 32;
const MAX_CATALOG_ROLES = 32;
const MAX_CATALOG_STRING_LEN = 128;
const OP_NAME_PATTERN = /^[a-z][a-z0-9._-]{0,63}$/;
/** Wire doc 7.3 and 7.7: the direct profile advertises exactly these channel-0 operations. */
const PROTOCOL_HOST_OPS: readonly string[] = [
    "route.open",
    "catalog.list",
    "host.shutdown",
    "host.status",
];
/** Wire doc 7.3: an unfiltered `catalog.list` returns exactly these modules in this order. */
const PROTOCOL_MODULE_IDS: readonly string[] = ["context", "synapse", "broca"];

function malformedCatalog(detail: string): HostCallError {
    return new HostCallError(
        "terminal",
        `catalog.list response rejected: ${detail}`,
        "malformed_control_response",
    );
}

function requireOpArray(value: unknown, field: string, allowEmpty: boolean): string[] {
    if (!Array.isArray(value)) throw malformedCatalog(`${field} is not an array`);
    if (!allowEmpty && value.length === 0) throw malformedCatalog(`${field} is empty`);
    if (value.length > MAX_CATALOG_OPS) throw malformedCatalog(`${field} exceeds the entry cap`);
    const seen = new Set<string>();
    for (const entry of value) {
        if (typeof entry !== "string" || !OP_NAME_PATTERN.test(entry)) {
            throw malformedCatalog(`${field} carries a non-operation entry`);
        }
        if (seen.has(entry)) throw malformedCatalog(`${field} carries a duplicate entry`);
        seen.add(entry);
    }
    return value as string[];
}

function parseHostStatusResponse(parsed: Record<string, unknown>): HostStatusSnapshot {
    if (parsed.op !== "host.status") {
        throw new HostCallError(
            "terminal",
            "host.status response rejected: operation mismatch",
            "malformed_control_response",
        );
    }
    const health = parsed.health;
    if (health !== "ok" && health !== "degraded" && health !== "failing") {
        throw new HostCallError(
            "terminal",
            "host.status response rejected: invalid health",
            "malformed_control_response",
        );
    }
    if (
        parsed.metrics === null ||
        typeof parsed.metrics !== "object" ||
        Array.isArray(parsed.metrics)
    ) {
        throw new HostCallError(
            "terminal",
            "host.status response rejected: metrics are not an object",
            "malformed_control_response",
        );
    }
    // Wire doc 7.6 requires `metrics.components` to be an object.
    const components = (parsed.metrics as Record<string, unknown>).components;
    if (components === null || typeof components !== "object" || Array.isArray(components)) {
        throw new HostCallError(
            "terminal",
            "host.status response rejected: metrics.components is not an object",
            "malformed_control_response",
        );
    }
    const sharedMemory = parsed.shared_memory;
    if (
        sharedMemory !== undefined &&
        (sharedMemory === null || typeof sharedMemory !== "object" || Array.isArray(sharedMemory))
    ) {
        throw new HostCallError(
            "terminal",
            "host.status response rejected: shared_memory is not an object",
            "malformed_control_response",
        );
    }
    return {
        health,
        metrics: parsed.metrics as Record<string, unknown>,
        ...(sharedMemory === undefined
            ? {}
            : { sharedMemory: sharedMemory as Record<string, unknown> }),
    };
}

function sameStringList(actual: readonly string[], expected: readonly string[]): boolean {
    return actual.length === expected.length && actual.every((entry, i) => entry === expected[i]);
}

/**
 * The decoder treats `catalog.list` as an open-shape control response: it ignores unknown fields but rejects missing, ill-typed, or out-of-bounds required fields.
 * The decoder rejects responses whose required exposed fields are absent, ill-typed, or out of bounds.
 * Treat `host_ops` and the module list as closed-shape: values differing from the protocol's fixed lists make the
 * daemon incompatible, not less capable.
 */
function parseCatalogResponse(parsed: Record<string, unknown>): CatalogSnapshot {
    const generation = parsed.generation;
    if (typeof generation !== "number" || !Number.isSafeInteger(generation) || generation < 0) {
        throw malformedCatalog("generation is not a nonnegative integer");
    }
    const hostOps = requireOpArray(parsed.host_ops, "host_ops", false);
    if (!sameStringList(hostOps, PROTOCOL_HOST_OPS)) {
        throw malformedCatalog(`host_ops must be exactly ${PROTOCOL_HOST_OPS.join(", ")}`);
    }
    const rawModules = parsed.modules;
    if (!Array.isArray(rawModules)) throw malformedCatalog("modules is not an array");
    if (rawModules.length > MAX_CATALOG_MODULES) {
        throw malformedCatalog("modules exceeds the entry cap");
    }
    const seenIds = new Set<string>();
    const modules: CatalogEntry[] = rawModules.map((raw) => {
        if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
            throw malformedCatalog("module entry is not an object");
        }
        const record = raw as Record<string, unknown>;
        const moduleId = record.module_id;
        if (
            typeof moduleId !== "string" ||
            moduleId.length === 0 ||
            moduleId.length > MAX_CATALOG_STRING_LEN
        ) {
            throw malformedCatalog("module_id is not a bounded nonempty string");
        }
        if (seenIds.has(moduleId)) throw malformedCatalog("duplicate module_id");
        seenIds.add(moduleId);
        const moduleVersion = record.module_version;
        if (
            typeof moduleVersion !== "string" ||
            moduleVersion.length === 0 ||
            moduleVersion.length > MAX_CATALOG_STRING_LEN
        ) {
            throw malformedCatalog("module_version is not a bounded nonempty string");
        }
        const roles = record.roles;
        if (!Array.isArray(roles) || roles.length > MAX_CATALOG_ROLES) {
            throw malformedCatalog("roles is not a bounded array");
        }
        const controlOps = requireOpArray(record.control_ops, "control_ops", true);
        return {
            module_id: moduleId,
            module_version: moduleVersion,
            roles,
            control_ops: controlOps,
        };
    });
    if (
        !sameStringList(
            modules.map((module) => module.module_id),
            PROTOCOL_MODULE_IDS,
        )
    ) {
        throw malformedCatalog(
            `modules must be exactly ${PROTOCOL_MODULE_IDS.join(", ")} in that order`,
        );
    }
    return { generation, hostOps, modules };
}

function toManagedCallError(error: unknown): HostCallError {
    if (error instanceof HostCallError) return error;
    if (error instanceof StaleRouteHandleError) {
        return new HostCallError(
            "not_sent",
            `managed request used a stale route handle${causeMessage(error)}`,
            error.code,
            error,
        );
    }
    return new HostCallError(
        "terminal",
        `managed call failed${causeMessage(error)}`,
        errorCode(error),
        error,
    );
}

function credentialFingerprintKey(identity: BindIdentity): string {
    return JSON.stringify(
        Object.entries(identity.credential_fingerprints ?? {})
            // Sort with UTF-16 code-unit comparison so the key does not depend on runtime collation.
            .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0)),
    );
}

/** A route bound with no identity never matches, so the caller reopens it. */
function sameCredentialFingerprints(current: BindIdentity, bound: BindIdentity | null): boolean {
    return bound !== null && credentialFingerprintKey(current) === credentialFingerprintKey(bound);
}

/**
 * The key is a JSON array so a delimiter byte inside one component can never make two distinct bindings share a slot.
 */
function routeCacheKey(
    target: Extract<RouteTarget, { kind: ManagedRouteKind }>,
    identity: BindIdentity,
    consumerIdentity: ConsumerIdentity | undefined,
): string {
    return JSON.stringify([
        target.kind,
        target.module_id,
        identity.project_root,
        identity.harness,
        identity.session,
        credentialFingerprintKey(identity),
        consumerIdentity ? [consumerIdentity.module_id, consumerIdentity.launch_nonce] : null,
    ]);
}
