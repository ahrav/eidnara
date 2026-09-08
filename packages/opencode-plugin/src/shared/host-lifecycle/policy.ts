/**
 * Shared lifecycle ownership policy: the one place that composes data-root
 * resolution, filesystem admission, the platform gate, bootstrap trust, the
 * native lifecycle transaction, and demand-start coalescing.
 *
 * Ownership rules (KTD13/KTD17): only `managed-default` connection origin may
 * reach {@link HostLifecyclePolicy.demandStart}; explicit connection files
 * and injected clients never construct a policy call. Concurrent managed
 * demands coalesce on one shared native start keyed by data root and startup
 * envelope; each caller races the shared promise against its own
 * signal/deadline, and a detaching caller never cancels the native work.
 *
 * Every operation returns one KTD12 v1 result object. Pre-native failures are
 * synthesized locally with the bounded no-follow root classifier; no raw
 * path, stderr text, or native error chain rides on any result.
 */

import hostRelease from "../../../../../release/host-release.json";
import type { AuthenticatedPeer, CatalogEntry } from "../host-client";
import { stableStringify } from "../stable-json";
import {
    checkPlatform,
    type LifecycleFailureReason,
    type PlatformGate,
    type PlatformReaders,
} from "./bootstrap";
import {
    COMPATIBILITY_STAGES,
    type CompatibilityInput,
    type CompatibilityStage,
    type CompatibilityVerdict,
    compatibilityStageIndex,
    evaluateCompatibility,
    type ObservedEpochs,
} from "./compatibility";
import {
    classifyPreNativeRoots,
    DAEMON_RESULT_SCHEMA,
    type DaemonCheck,
    type DaemonCommand,
    type DaemonReadiness,
    type DaemonReason,
    type DaemonResultV1,
    type DaemonState,
    fixedStateForReason,
    preNativeState,
    probeFallbackVerdict,
    reasonPrecedence,
    remediationForReason,
} from "./contract";
import {
    NativeLaunchError,
    type NativeLaunchTarget,
    type NativeLifecycleCommand,
    type NativeStartupEnvelope,
    runNativeLifecycle,
} from "./native-launcher";
import { type ConnectionOrigin, mayDemandStart } from "./ownership";
import { type AdmissionIo, admitLifecycleFilesystem, resolveLifecycleDataRoot } from "./paths";

/** Managed Eidnara demand waits at most this long for storage (R11). */
export const STORAGE_HARD_BUDGET_MS = 5_000;
/** Fresh Linux request-to-authenticated-transport outer aggregate (hard). */
export const OUTER_AGGREGATE_MS = 60_000;
export function aggregateForTarget(_target: "linux-x64-gnu"): number {
    return OUTER_AGGREGATE_MS;
}

export type LifecycleCommand = "start" | "stop" | "restart" | "status" | "doctor";

export type StorageReadiness = "ready" | "starting" | "unavailable";

export type { CompatibilityStage } from "./compatibility";

export interface CompatibilitySnapshot {
    authenticatedPeer: AuthenticatedPeer;
    /** Compatibility alias retained for callers reading the staged snapshot. */
    authenticatedDaemonVersion?: string;
    /** Compatibility alias retained for callers fencing from the staged snapshot. */
    authenticatedDaemonId?: Uint8Array;
    catalog: CatalogEntry[];
    epochs: ObservedEpochs;
    /** Last stage reached by the ordered authenticated compatibility probe. */
    evaluatedThrough?: CompatibilityStage;
}

export interface ObservationalHealth extends CompatibilitySnapshot {
    readiness: DaemonReadiness;
}

/** Thrown after ring attachment and authentication when a control probe fails. */
export class ReadinessProbeControlError extends Error {
    constructor(cause: unknown) {
        super("readiness control probe failed", { cause });
        this.name = "ReadinessProbeControlError";
    }
}

function compatibilityInput(snapshot: CompatibilitySnapshot): CompatibilityInput {
    return {
        authenticatedPeer: snapshot.authenticatedPeer,
        catalog: snapshot.catalog,
        epochs: snapshot.epochs,
    };
}

/**
 * Elapsed-time source for every lifecycle budget.
 *
 * `Date.now()` is a wall clock and can step in either direction: a backward
 * correction makes elapsed time negative and hands the native child more than
 * its platform's qualified aggregate, while a forward correction expires a live
 * request that has barely started. Budgets are durations, so they are measured
 * on a monotonic timeline — the same basis, and the same reason, as the client's
 * `Deadline`.
 *
 * `performance.now()` does not advance across system suspend. That is the right
 * trade here: it can never over-grant, and expiring on resume would need an
 * explicit second signal rather than a clock that also jumps for timezone and
 * NTP corrections.
 */
export function monotonicNow(): number {
    return performance.now();
}

/** `setTimeout` coerces delays above this limit to 1 ms, detaching waiters before their deadlines. */
const MAX_TIMER_DELAY_MS = 2_147_483_647;

function timerDelay(deadlineMs: number): number {
    return Math.min(deadlineMs, MAX_TIMER_DELAY_MS);
}

function nativeChildRan(error: unknown): boolean {
    return error instanceof NativeLaunchError && error.childMayHaveActed;
}

/** `lifecycle_busy` and `harness_unavailable` return before the binary spawns or stops anything. commentlint: allow(JUDGE) */
const DAEMON_AS_FOUND_REASONS: ReadonlySet<DaemonReason> = new Set([
    "already_running",
    "already_stopped",
    "lifecycle_busy",
    "harness_unavailable",
]);

export class WaiterDetachedError extends Error {
    /**
     * `ETIMEDOUT` for a deadline detach, so callers that classify retryability on `code`
     * see a timeout. The class itself still tells the transport this was one caller's
     * deadline and must not arm the transport-wide connect backoff.
     */
    readonly code?: string;

    constructor(readonly cause_kind: "aborted" | "deadline") {
        super(`managed startup waiter detached: ${cause_kind}`);
        this.name = "WaiterDetachedError";
        if (cause_kind === "deadline") {
            this.code = "ETIMEDOUT";
        }
    }
}

export interface LifecyclePolicyOptions {
    env?: Record<string, string | undefined>;
    /**
     * The trusted launch target for native lifecycle commands: a retained
     * verified bootstrap descriptor in production, or the explicit test-only
     * binary injection used by this repo's dev/test path. `null` means no
     * trusted retained current-release bootstrap exists, so observational
     * commands use the no-probe classifier and mutating commands fail with
     * the package-path reason.
     */
    launchTarget?: NativeLaunchTarget | null;
    /** Pre-resolve failure for mutating commands, already reduced to a closed reason. */
    bootstrapFailure?: LifecycleFailureReason;
    platformReaders?: PlatformReaders;
    admissionIo?: AdmissionIo;
    /**
     * Post-transport storage probe used by managed Eidnara demand.
     * The default reports `ready` for explicit and test-only policy instances.
     *
     * `expectedDaemonId` is the incarnation compatibility just certified. A probe
     * that cannot observe that incarnation must reject rather than report a
     * reading from another one, because the caller fences its traffic to the
     * certified identity and would act on readiness it will never reach.
     * U4 wires the real Eidnara status call. There is no
     * permissive default: an unset probe reports `unavailable`, because a
     * default of `ready` would authorize application bodies against a daemon
     * whose storage state was never examined. Explicit CLI flows are
     * unaffected — they never reach `demandStart`.
     *
     * `signal` is the demanding caller's own; it releases this caller's share of
     * any coalesced observation and never cancels work another caller awaits.
     */
    storageProbe?: (
        budgetMs: number,
        expectedDaemonId?: Uint8Array,
        signal?: AbortSignal,
    ) => Promise<StorageReadiness>;
    /**
     * Authenticated daemon, catalog, and Eidnara epoch snapshot for demand.
     * Managed demand fails closed with `native_probe_unavailable` when this is
     * unset: a start whose incarnation was never certified authorizes nothing.
     */
    compatibilityProbe?: (budgetMs: number, signal?: AbortSignal) => Promise<CompatibilitySnapshot>;
    /** Authenticated route-free component health for status and doctor. */
    readinessProbe?: (budgetMs: number) => Promise<ObservationalHealth>;
    /** Dev/test payload directory forwarded to native start/restart. */
    payloadDir?: string;
    /** Parent-trusted payload manifest digest paired with `payloadDir`. */
    payloadManifestDigest?: string;
    /** Deferred certified package lookup after native current validation says missing. */
    payloadDirFallback?: () => string | null;
    /** Credential-only fallback used by CLI start/restart callers. */
    defaultStartupEnvelope?: NativeStartupEnvelope;
    outerAggregateMs?: number;
}

/**
 * Synthesize a pre-native v1 result.
 *
 * `effectsKnown` distinguishes a failure that provably committed nothing —
 * the native binary was never invoked — from one whose native transaction was
 * killed mid-flight and whose outcome is therefore unknown. Only the former
 * may state `stop_committed`/`start_committed`; the latter reports `null`,
 * because asserting `false` for a SIGKILLed restart would tell an operator the
 * old incarnation is still serving when the stop may already have committed.
 */
function localResult(
    command: LifecycleCommand,
    ok: boolean,
    state: DaemonState,
    reason: DaemonReason,
    effectsKnown = true,
): DaemonResultV1 {
    return {
        schema: DAEMON_RESULT_SCHEMA,
        command,
        ok,
        state,
        reason,
        remediation: remediationForReason(reason),
        effects:
            command === "restart" && effectsKnown
                ? { stop_committed: false, start_committed: false }
                : null,
        readiness: null,
        checks: [],
        versions: {
            release: hostRelease.release.version,
            proof: null,
            daemon: null,
            context: null,
            synapse: null,
            broca: null,
        },
    };
}

/**
 * Reason for a native command whose child was killed at the deadline, keyed by
 * the caller-facing command. `status` and `doctor` run no startup or shutdown:
 * the killed child was the read-only probe, so the daemon was left unobserved
 * rather than left mid-transaction.
 */
const TIMEOUT_REASON: Record<LifecycleCommand, DaemonReason> = {
    start: "startup_timeout",
    restart: "startup_timeout",
    stop: "shutdown_timeout",
    status: "native_probe_unavailable",
    doctor: "native_probe_unavailable",
};

/** `shutdown_timeout` permits only `stopping`; other timeout reasons use the root classifier's state. */
function timeoutResult(
    command: LifecycleCommand,
    root: string,
    effectsKnown: boolean,
): DaemonResultV1 {
    const reason = TIMEOUT_REASON[command];
    const state = fixedStateForReason(reason) ?? preNativeState(classifyPreNativeRoots(root));
    return localResult(command, false, state, reason, effectsKnown);
}

/** A native start without an authenticated compatibility snapshot must not authorize traffic against its daemon. */
function unprovenCompatibility(result: DaemonResultV1): DaemonResultV1 {
    return {
        ...result,
        ok: false,
        reason: "native_probe_unavailable",
        remediation: remediationForReason("native_probe_unavailable"),
        versions: { ...result.versions, proof: null },
    };
}

/**
 * Native `start` answers `harness_unavailable` for a changed harness or credential set on a running daemon, so demands with different envelopes are different requests and must not share one result. commentlint: allow(JUDGE)
 * The identity is the JSON the child receives with keys normalized, so wire-identical envelopes coalesce however their callers built the object. commentlint: allow(JUDGE)
 */
function envelopeIdentity(envelope: NativeStartupEnvelope | undefined): string {
    if (envelope === undefined) return "";
    const wire = JSON.stringify(envelope);
    return wire === undefined ? "" : stableStringify(JSON.parse(wire));
}

export interface DemandStartRequest {
    origin: ConnectionOrigin;
    capability: "context" | "synapse";
    signal?: AbortSignal;
    deadlineMs?: number;
    startupEnvelope?: NativeStartupEnvelope;
}

export interface DemandStartOutcome {
    result: DaemonResultV1;
    /**
     * Storage readiness at return time for `context` capability.
     * Callers must send no Rust application body unless this is `ready`.
     */
    storage: StorageReadiness | null;
    /** Authenticated incarnation that passed compatibility; bind application traffic to it. */
    authenticatedDaemonId?: Uint8Array;
}

export class HostLifecyclePolicy {
    private readonly env: Record<string, string | undefined>;
    private readonly launchTarget: NativeLaunchTarget | null;
    private readonly bootstrapFailure: LifecycleFailureReason | undefined;
    private readonly platformReaders: PlatformReaders | undefined;
    private readonly admissionIo: AdmissionIo | undefined;
    private readonly storageProbe: (
        budgetMs: number,
        expectedDaemonId?: Uint8Array,
        signal?: AbortSignal,
    ) => Promise<StorageReadiness>;
    private readonly compatibilityProbe:
        | ((budgetMs: number, signal?: AbortSignal) => Promise<CompatibilitySnapshot>)
        | undefined;
    private readonly readinessProbe:
        | ((budgetMs: number) => Promise<ObservationalHealth>)
        | undefined;
    private readonly payloadDir: string | undefined;
    private readonly payloadManifestDigest: string | undefined;
    private readonly payloadDirFallback: (() => string | null) | undefined;
    private readonly defaultStartupEnvelope: NativeStartupEnvelope | undefined;
    private readonly outerAggregateMs: number | undefined;
    private readonly inflightStarts = new Map<string, Promise<DaemonResultV1>>();
    /** One in-flight compatibility probe per data root, keyed independently of capability. */
    private readonly inflightCompatibility = new Map<
        string,
        { generation: number; snapshot: Promise<CompatibilitySnapshot> }
    >();
    /** Advances after each native mutation unless the daemon is provably as the command found it; see `invokeMutation`. commentlint: allow(JUDGE) */
    private lifecycleGeneration = 0;
    private platformGateResult: PlatformGate | undefined;

    constructor(options: LifecyclePolicyOptions = {}) {
        this.env = options.env ?? process.env;
        this.launchTarget = options.launchTarget ?? null;
        this.bootstrapFailure = options.bootstrapFailure;
        this.platformReaders = options.platformReaders;
        this.admissionIo = options.admissionIo;
        // Fail closed: an unwired probe must not assert readiness.
        this.storageProbe = options.storageProbe ?? (async () => "unavailable");
        this.compatibilityProbe = options.compatibilityProbe;
        this.readinessProbe = options.readinessProbe;
        this.payloadDir = options.payloadDir;
        this.payloadManifestDigest = options.payloadManifestDigest;
        this.payloadDirFallback = options.payloadDirFallback;
        this.defaultStartupEnvelope = options.defaultStartupEnvelope;
        this.outerAggregateMs = options.outerAggregateMs;
    }

    /** Count of live coalesced startups; test observability only. */
    get inflightStartCount(): number {
        return this.inflightStarts.size;
    }

    async start(
        startupEnvelope: NativeStartupEnvelope | undefined = this.defaultStartupEnvelope,
    ): Promise<DaemonResultV1> {
        return this.mutatingCommand("start", startupEnvelope);
    }

    async stop(): Promise<DaemonResultV1> {
        return this.mutatingCommand("stop");
    }

    /** One native restart transaction; never emulated as TS stop+start. */
    async restart(
        startupEnvelope: NativeStartupEnvelope | undefined = this.defaultStartupEnvelope,
    ): Promise<DaemonResultV1> {
        return this.mutatingCommand("restart", startupEnvelope);
    }

    async status(): Promise<DaemonResultV1> {
        return this.observationalCommand("status");
    }

    async doctor(): Promise<DaemonResultV1> {
        return this.observationalCommand("doctor");
    }

    /**
     * Managed demand-start with KTD17 coalescing. Only `managed-default`
     * origin is accepted; the shared native start is keyed by data root,
     * callers race it against their own signal/deadline, and a settled promise
     * is evicted so no rejection becomes a permanent latch. For the
     * `context` capability, the outcome additionally reports storage
     * readiness after waiting at most the 5-second hard budget.
     */
    async demandStart(request: DemandStartRequest): Promise<DemandStartOutcome> {
        if (!mayDemandStart(request.origin)) {
            throw new Error(`connection origin ${request.origin} is lifecycle-neutral`);
        }
        const startedAt = monotonicNow();
        // Validated at entry, for every caller, before the shared start is even
        // looked up. A caller with no live interest must not create a start —
        // `start()`'s synchronous prefix reaches `spawn()` before any await, so
        // it would launch a mutating child nobody is waiting for. And a caller
        // *joining* an existing start must not be admitted either: `raceWaiter`
        // subtracts elapsed time and `NaN` stays `NaN`, while `setTimeout`
        // coerces both `NaN` and `Infinity` to a 1ms delay, so a non-finite
        // budget yields either a ~1ms detach or — if the shared start settles
        // within one microtask drain — a result adopted on an invalid budget.
        // Identical input would then resolve or reject depending only on whether
        // another demand happened to be in flight.
        //
        // Rejecting a caller is not cancelling: the shared promise stays in the
        // map untouched, so every other waiter is unaffected, which is the
        // detach-only guarantee this design actually requires.
        if (request.signal?.aborted) throw new WaiterDetachedError("aborted");
        if (
            request.deadlineMs !== undefined &&
            (!Number.isFinite(request.deadlineMs) || request.deadlineMs <= 0)
        ) {
            throw new WaiterDetachedError("deadline");
        }
        const callerDeadlineAt =
            request.deadlineMs === undefined ? undefined : startedAt + request.deadlineMs;
        const rootResolution = resolveLifecycleDataRoot(this.env);
        // The data root alone identifies the host: `start()` takes no
        // capability and one daemon serves them all, so keying on capability
        // would launch a second native start that only collides with the
        // first on the transaction lock.
        const rootKey = rootResolution.ok ? rootResolution.root : "\u0000no-root";
        const startupEnvelope = request.startupEnvelope ?? this.defaultStartupEnvelope;
        const key = `${rootKey}\u0000${envelopeIdentity(startupEnvelope)}`;
        // Serializing the envelope ran caller-supplied code and may have spent
        // the caller's deadline or the policy aggregate; neither may then spawn.
        if (callerDeadlineAt !== undefined && monotonicNow() >= callerDeadlineAt) {
            throw new WaiterDetachedError("deadline");
        }
        const aggregateMs = this.compatibilityAggregateMs();
        const aggregateDeadlineAt = startedAt + aggregateMs;
        if (rootResolution.ok && monotonicNow() >= aggregateDeadlineAt) {
            return { result: timeoutResult("start", rootResolution.root, true), storage: null };
        }
        let shared = this.inflightStarts.get(key);
        if (!shared) {
            shared = this.start(startupEnvelope);
            this.inflightStarts.set(key, shared);
            void shared
                .catch(() => {})
                .finally(() => {
                    if (this.inflightStarts.get(key) === shared) this.inflightStarts.delete(key);
                });
        }
        const result = await this.raceDetached(shared, request.signal, callerDeadlineAt);
        if (!result.ok) {
            return { result, storage: null };
        }
        if (callerDeadlineAt !== undefined && monotonicNow() >= callerDeadlineAt) {
            throw new WaiterDetachedError("deadline");
        }
        // Same fail-closed rule as an unset storage probe: with no compatibility
        // probe the incarnation is never certified, and the storage probe, which
        // fences on the certified id, has nothing to fence on.
        const compatibilityProbe = this.compatibilityProbe;
        if (compatibilityProbe === undefined) {
            return { result: unprovenCompatibility(result), storage: null };
        }
        // The shared start consumes part of this demand's aggregate budget. The
        // shared probe keeps the full aggregate so a late joiner is not truncated.
        if (monotonicNow() >= aggregateDeadlineAt) {
            return { result: unprovenCompatibility(result), storage: null };
        }
        let snapshot: CompatibilitySnapshot;
        try {
            snapshot = await this.raceWithinPolicy(
                this.sharedCompatibility(compatibilityProbe, rootKey, aggregateMs),
                request.signal,
                callerDeadlineAt,
                aggregateDeadlineAt,
            );
        } catch (error) {
            // Detachment is the caller's own deadline or signal and stays a
            // thrown control outcome. Any other probe failure is an unproven
            // compatibility claim, so it becomes a typed closed result rather
            // than an unclassified rejection callers cannot act on.
            if (error instanceof WaiterDetachedError) throw error;
            return { result: unprovenCompatibility(result), storage: null };
        }
        const applied = this.applyCompatibility(result, snapshot);
        const compatibleResult = applied.result;
        if (!applied.verdict.ok) return { result: compatibleResult, storage: null };
        const authenticatedDaemonId = Uint8Array.from(snapshot.authenticatedPeer.daemonId);
        if (callerDeadlineAt !== undefined && monotonicNow() >= callerDeadlineAt) {
            throw new WaiterDetachedError("deadline");
        }
        if (request.capability !== "context") {
            return { result: compatibleResult, storage: null, authenticatedDaemonId };
        }
        // `storageDeadlineAt` is fixed before `storageProbe` runs, so synchronous probe work consumes its hard budget and any caller deadline.
        const storageDeadlineAt = monotonicNow() + STORAGE_HARD_BUDGET_MS;
        const storageBudget =
            callerDeadlineAt === undefined
                ? STORAGE_HARD_BUDGET_MS
                : Math.min(STORAGE_HARD_BUDGET_MS, callerDeadlineAt - monotonicNow());
        let storage: StorageReadiness;
        try {
            storage = await this.raceWithinPolicy(
                this.storageProbe(storageBudget, authenticatedDaemonId, request.signal),
                request.signal,
                callerDeadlineAt,
                storageDeadlineAt,
            );
        } catch (error) {
            if (error instanceof WaiterDetachedError) throw error;
            // Compatibility is already proven. A failed storage observation
            // cannot erase that proof; it only means this demand may not publish
            // application traffic.
            return { result: compatibleResult, storage: "unavailable", authenticatedDaemonId };
        }
        return { result: compatibleResult, storage, authenticatedDaemonId };
    }

    /**
     * One compatibility probe per data root in flight. The snapshot describes the
     * daemon incarnation, not the requesting capability, so concurrent demands
     * share one connection and one `catalog.list`/`host.status` pair instead of
     * each opening its own. The shared probe carries no caller signal; callers
     * bound their own wait with `raceDetached`, so one detaching caller cannot
     * cancel the probe another is still awaiting.
     *
     * Its budget is the policy's own aggregate, never the creating caller's
     * remaining deadline: a nearly expired waiter arriving first would otherwise
     * mint a probe too short for the long-lived waiters that join it, and they
     * would read that truncated failure as an unproven compatibility claim while
     * still holding ample time.
     *
     * The shared promise itself settles by that aggregate even if the probe
     * ignores its budget. Eviction runs on settlement, so an unbounded promise
     * would never leave the map and every later demand for the root would join
     * a probe that can no longer answer.
     *
     * A probe outlived by a native mutation is rejected and never joined: the daemon it authenticated may no longer be the one serving. commentlint: allow(JUDGE)
     */
    private sharedCompatibility(
        probe: (budgetMs: number, signal?: AbortSignal) => Promise<CompatibilitySnapshot>,
        root: string,
        budgetMs: number,
    ): Promise<CompatibilitySnapshot> {
        const existing = this.inflightCompatibility.get(root);
        if (existing && existing.generation === this.lifecycleGeneration) return existing.snapshot;
        const generation = this.lifecycleGeneration;
        const deadlineAt = monotonicNow() + budgetMs;
        const snapshot = this.raceWithinPolicy(
            probe(budgetMs),
            undefined,
            undefined,
            deadlineAt,
        ).then((observed) => {
            if (this.lifecycleGeneration !== generation) {
                throw new Error("daemon incarnation changed during the compatibility probe");
            }
            return observed;
        });
        const entry = { generation, snapshot };
        this.inflightCompatibility.set(root, entry);
        const evict = (): void => {
            if (this.inflightCompatibility.get(root) === entry) {
                this.inflightCompatibility.delete(root);
            }
        };
        void snapshot.then(evict, evict);
        return snapshot;
    }

    /** `platformReaders` is `readonly`, so the gate runs its readers once per policy; every later caller reads the memo. commentlint: allow(JUDGE) */
    private platformGate(): PlatformGate {
        this.platformGateResult ??= checkPlatform(this.platformReaders);
        return this.platformGateResult;
    }

    private compatibilityAggregateMs(): number {
        if (this.outerAggregateMs !== undefined) return this.outerAggregateMs;
        const platform = this.platformGate();
        return platform.ok ? aggregateForTarget(platform.target) : OUTER_AGGREGATE_MS;
    }

    /** A policy deadline is not caller detachment, so its expiry surfaces as a plain error. */
    private raceWithinPolicy<T>(
        shared: Promise<T>,
        signal: AbortSignal | undefined,
        callerDeadlineAt: number | undefined,
        policyDeadlineAt: number,
    ): Promise<T> {
        const callerBound = callerDeadlineAt !== undefined && callerDeadlineAt <= policyDeadlineAt;
        return this.raceDetached(
            shared,
            signal,
            callerBound ? callerDeadlineAt : policyDeadlineAt,
        ).catch((error: unknown) => {
            if (
                error instanceof WaiterDetachedError &&
                error.cause_kind === "deadline" &&
                !callerBound
            ) {
                throw new Error("policy budget expired before the probe answered");
            }
            throw error;
        });
    }

    /** `deadlineAt` is absolute in the `monotonicNow()` timebase. */
    private raceDetached<T>(
        shared: Promise<T>,
        signal: AbortSignal | undefined,
        deadlineAt: number | undefined,
    ): Promise<T> {
        if (!signal && deadlineAt === undefined) return shared;
        return new Promise<T>((resolve, reject) => {
            let settled = false;
            let timer: ReturnType<typeof setTimeout> | null = null;
            const detach = (kind: "aborted" | "deadline"): void => {
                if (settled) return;
                settled = true;
                if (timer !== null) clearTimeout(timer);
                signal?.removeEventListener("abort", onAbort);
                // A detached waiter does not cancel `shared`; other waiters may still need it.
                reject(new WaiterDetachedError(kind));
            };
            const onAbort = (): void => detach("aborted");
            if (signal) {
                if (signal.aborted) {
                    detach("aborted");
                    return;
                }
                signal.addEventListener("abort", onAbort, { once: true });
            }
            if (deadlineAt !== undefined) {
                // A timer of 0 fires in a later macrotask, so an already-settled `shared` would resolve this waiter through the microtask queue first and hand it a result it had no time left to wait for; a probe's synchronous prefix ran before this call, so its cost lands here too. commentlint: allow(JUDGE)
                const delayMs = deadlineAt - monotonicNow();
                if (delayMs <= 0) {
                    detach("deadline");
                    return;
                }
                timer = setTimeout(() => detach("deadline"), timerDelay(delayMs));
            }
            shared.then(
                (value) => {
                    if (settled) return;
                    // Reject values delivered at or after `deadlineAt` even if their microtask runs before the timer.
                    if (deadlineAt !== undefined && monotonicNow() >= deadlineAt) {
                        detach("deadline");
                        return;
                    }
                    settled = true;
                    if (timer !== null) clearTimeout(timer);
                    signal?.removeEventListener("abort", onAbort);
                    resolve(value);
                },
                (error: unknown) => {
                    if (settled) return;
                    // A late rejection is the deadline's outcome, not the probe's; classifying it as a probe failure would let it stand in for a detachment. commentlint: allow(JUDGE)
                    if (deadlineAt !== undefined && monotonicNow() >= deadlineAt) {
                        detach("deadline");
                        return;
                    }
                    settled = true;
                    if (timer !== null) clearTimeout(timer);
                    signal?.removeEventListener("abort", onAbort);
                    reject(error instanceof Error ? error : new Error(String(error)));
                },
            );
        });
    }

    // ------------------------------------------------------------------
    // Shared preflight and native invocation.
    // ------------------------------------------------------------------

    /**
     * Root resolution, filesystem admission, and the platform gate: the
     * pre-native checks every command shares.
     *
     * Observational commands gate on the platform too. A host outside the
     * supported target table has no retained-descriptor exec path, so probing
     * it or answering with the no-probe classifier would report a daemon state
     * for a host the release cannot run on at all.
     */
    private preflight(
        command: LifecycleCommand,
    ): { ok: true; root: string; deadlineMs: number } | { ok: false; result: DaemonResultV1 } {
        // Preflight cost counts against the request-to-transport aggregate.
        const startedAt = monotonicNow();
        const rootResolution = resolveLifecycleDataRoot(this.env);
        if (!rootResolution.ok) {
            return { ok: false, result: localResult(command, false, "unavailable", "no_data_dir") };
        }
        const root = rootResolution.root;
        const admission = admitLifecycleFilesystem(root, this.admissionIo);
        if (!admission.ok) {
            const state = preNativeState(classifyPreNativeRoots(root));
            return {
                ok: false,
                result: localResult(command, false, state, admission.reason),
            };
        }
        const platform = this.platformGate();
        if (!platform.ok) {
            const state = preNativeState(classifyPreNativeRoots(root));
            return {
                ok: false,
                result: localResult(command, false, state, platform.reason),
            };
        }
        // The gate already resolved which qualified target this host is, so the
        // aggregate comes from that rather than from a Linux-shaped default.
        const aggregate = this.outerAggregateMs ?? aggregateForTarget(platform.target);
        const deadlineMs = aggregate - (monotonicNow() - startedAt);
        if (deadlineMs <= 0) {
            // Preflight consumed the whole budget, so the operation is out of
            // time before the child exists. That is this command's timeout, not
            // an internal error: nothing was spawned, so a restart reports no
            // committed effects rather than unknown ones.
            return { ok: false, result: timeoutResult(command, root, true) };
        }
        return { ok: true, root, deadlineMs };
    }

    private async mutatingCommand(
        command: "start" | "stop" | "restart",
        startupEnvelope?: NativeStartupEnvelope,
    ): Promise<DaemonResultV1> {
        const preflight = this.preflight(command);
        if (!preflight.ok) return preflight.result;
        if (this.bootstrapFailure !== undefined) {
            const state = preNativeState(classifyPreNativeRoots(preflight.root));
            return localResult(command, false, state, this.bootstrapFailure);
        }
        if (this.launchTarget === null) {
            const state = preNativeState(classifyPreNativeRoots(preflight.root));
            return localResult(command, false, state, "native_payload_missing");
        }
        const { result, daemonMayHaveChanged } = await this.invokeMutation(
            command,
            preflight,
            this.launchTarget,
            startupEnvelope,
        );
        if (daemonMayHaveChanged) this.lifecycleGeneration += 1;
        return result;
    }

    /** `daemonMayHaveChanged` is false only when no child ran or the child answered that it acted on nothing. commentlint: allow(JUDGE) */
    private async invokeMutation(
        command: "start" | "stop" | "restart",
        preflight: { root: string; deadlineMs: number },
        launchTarget: NativeLaunchTarget,
        startupEnvelope: NativeStartupEnvelope | undefined,
    ): Promise<{ result: DaemonResultV1; daemonMayHaveChanged: boolean }> {
        try {
            // The aggregate is one request-to-transport bound for the whole
            // command, not per native invocation. The certified-package lookup
            // and a first launch that answers `native_payload_missing` both spend
            // from it, so each invocation gets the residual. Handing
            // `preflight.deadlineMs` to both would let a fallback retry run a
            // second full aggregate — twice the budget the platform was
            // qualified for.
            const startedAt = monotonicNow();
            const remaining = (): number => preflight.deadlineMs - (monotonicNow() - startedAt);
            const invoke = (payloadDir: string | undefined, deadlineMs: number) =>
                runNativeLifecycle(launchTarget, {
                    command: command as NativeLifecycleCommand,
                    deadlineMs,
                    dataRoot: preflight.root,
                    ...(payloadDir !== undefined && command !== "stop" ? { payloadDir } : {}),
                    ...(command !== "stop" && this.payloadManifestDigest !== undefined
                        ? { payloadManifestDigest: this.payloadManifestDigest }
                        : {}),
                    ...(startupEnvelope === undefined || command === "stop"
                        ? {}
                        : { envelope: startupEnvelope }),
                });
            let selectedPayloadDir = this.payloadDir;
            if (
                command === "restart" &&
                selectedPayloadDir === undefined &&
                this.payloadDirFallback !== undefined
            ) {
                selectedPayloadDir = this.payloadDirFallback() ?? undefined;
            }
            const firstBudget = remaining();
            if (firstBudget <= 0) {
                // The lookup consumed the command's budget before any child
                // existed, so nothing was spawned and nothing committed.
                return {
                    result: timeoutResult(command, preflight.root, true),
                    daemonMayHaveChanged: false,
                };
            }
            let native = await invoke(selectedPayloadDir, firstBudget);
            if (
                command === "start" &&
                this.payloadDir === undefined &&
                native.reason === "native_payload_missing" &&
                this.payloadDirFallback !== undefined
            ) {
                const fallback = this.payloadDirFallback();
                if (fallback !== null) {
                    const retryBudget = remaining();
                    // With no budget left the retry cannot be attempted, and the
                    // first launch already answered. Reporting its real result
                    // beats replacing a completed observation with a synthetic
                    // timeout.
                    if (retryBudget > 0) native = await invoke(fallback, retryBudget);
                }
            }
            return {
                result: this.relabel(native, command, command),
                daemonMayHaveChanged: !DAEMON_AS_FOUND_REASONS.has(native.reason),
            };
        } catch (error) {
            return {
                result: this.launchFailure(command, preflight.root, error),
                daemonMayHaveChanged: nativeChildRan(error),
            };
        }
    }

    private async observationalCommand(command: "status" | "doctor"): Promise<DaemonResultV1> {
        const preflight = this.preflight(command);
        if (!preflight.ok) return preflight.result;
        if (this.launchTarget === null) {
            // No trusted retained current-release bootstrap: only the bounded
            // no-follow classifier may speak, and it authorizes nothing.
            const verdict = probeFallbackVerdict(classifyPreNativeRoots(preflight.root));
            const ok = false;
            return localResult(command, ok, verdict.state, verdict.reason);
        }
        try {
            const startedAt = monotonicNow();
            const native = await runNativeLifecycle(this.launchTarget, {
                command: "probe",
                deadlineMs: preflight.deadlineMs,
                dataRoot: preflight.root,
            });
            const relabeled = this.relabel(native, "status", command);
            if (
                !relabeled.ok ||
                relabeled.state !== "running" ||
                this.readinessProbe === undefined
            ) {
                return relabeled;
            }
            // The readiness probe shares the command's aggregate with the probe
            // child that just ran, so it gets only what that child left behind.
            // An exhausted budget means there is nothing left to observe with:
            // granting a 1ms floor would start a probe that can only fail.
            const deadlineAt = startedAt + preflight.deadlineMs;
            const remaining = deadlineAt - monotonicNow();
            if (remaining <= 0) return relabeled;
            // A readiness failure must not erase an observation that already
            // succeeded. Letting it reach the outer `catch` would answer
            // `internal_error` for a daemon this call verifiably observed, so a
            // rejected probe degrades to `relabeled`. `raceDetached` enforces `remaining` when the probe ignores its argument. commentlint: allow(JUDGE)
            let observed: ObservationalHealth;
            try {
                observed = await this.raceDetached(
                    this.readinessProbe(remaining),
                    undefined,
                    deadlineAt,
                );
            } catch {
                return relabeled;
            }
            const { result: compatible } = this.applyCompatibility(relabeled, observed);
            const checksById = new Map(
                compatible.checks.map((check) => [check.id, check] as const),
            );
            const addCheck = (
                id:
                    | "readiness.transport"
                    | "readiness.storage"
                    | "readiness.synapse"
                    | "readiness.kernel",
                record: NonNullable<DaemonReadiness[keyof DaemonReadiness]>,
            ): void => {
                // A `ready` component with a non-healthy reason is degraded but
                // still serving, so it warns rather than fails.
                const status =
                    record.state === "ready"
                        ? record.reason === "healthy"
                            ? "pass"
                            : "warn"
                        : record.state === "unsupported"
                          ? "skip"
                          : "fail";
                checksById.set(id, {
                    id,
                    status,
                    reason: record.reason,
                    remediation: remediationForReason(record.reason),
                });
            };
            if (observed.readiness.transport) {
                addCheck("readiness.transport", observed.readiness.transport);
            }
            if (observed.readiness.storage) {
                addCheck("readiness.storage", observed.readiness.storage);
            }
            if (observed.readiness.synapse) {
                addCheck("readiness.synapse", observed.readiness.synapse);
            }
            if (observed.readiness.kernel) {
                addCheck("readiness.kernel", observed.readiness.kernel);
            }
            const checks = [...checksById.values()].sort((left, right) =>
                left.id.localeCompare(right.id),
            );
            checks.sort((left, right) => left.id.localeCompare(right.id));
            const failed = checks
                .filter((check) => check.status === "fail")
                .reduce<DaemonCheck | undefined>((winner, check) => {
                    if (!winner) return check;
                    const winning = reasonPrecedence(winner.reason) ?? Number.MAX_SAFE_INTEGER;
                    const candidate = reasonPrecedence(check.reason) ?? Number.MAX_SAFE_INTEGER;
                    return candidate < winning ? check : winner;
                }, undefined);
            const reason = compatible.ok ? (failed?.reason ?? "healthy") : compatible.reason;
            const remediation = compatible.ok
                ? (failed?.remediation ?? null)
                : compatible.remediation;
            const ok = compatible.ok && failed === undefined;
            const state = ok ? compatible.state : (fixedStateForReason(reason) ?? compatible.state);
            return {
                ...compatible,
                command,
                ok,
                state,
                reason,
                remediation,
                readiness: observed.readiness,
                checks,
            };
        } catch (error) {
            return this.launchFailure(command, preflight.root, error);
        }
    }

    private applyCompatibility(
        result: DaemonResultV1,
        snapshot: CompatibilitySnapshot,
    ): { result: DaemonResultV1; verdict: CompatibilityVerdict } {
        const input = compatibilityInput(snapshot);
        const verdict = evaluateCompatibility(input);
        const evaluatedThroughIndex = compatibilityStageIndex(
            snapshot.evaluatedThrough ?? "epochs",
        );
        const checksById = new Map(result.checks.map((check) => [check.id, check] as const));
        for (const [index, stage] of COMPATIBILITY_STAGES.entries()) {
            // Only stages the probe actually reached are reported; a check for
            // an unevaluated stage would assert an observation never made.
            if (index > evaluatedThroughIndex) continue;
            const stageVerdict = stage.evaluate(input);
            const reason = stageVerdict.ok ? "healthy" : stageVerdict.reason;
            checksById.set(stage.checkId, {
                id: stage.checkId,
                status: stageVerdict.ok ? "pass" : "fail",
                reason,
                remediation: remediationForReason(reason),
            });
        }
        const moduleVersion = (moduleId: string): string | null =>
            snapshot.catalog.find((entry) => entry.module_id === moduleId)?.module_version ?? null;
        const ok = result.ok && verdict.ok;
        // Only a successful start or restart vouches for the running code; an
        // observation reports the authenticated version without claiming proof.
        const proof =
            ok && (result.command === "start" || result.command === "restart") ? "current" : null;
        return {
            verdict,
            result: {
                ...result,
                ok,
                reason: verdict.ok ? result.reason : verdict.reason,
                remediation: verdict.ok ? result.remediation : remediationForReason(verdict.reason),
                checks: [...checksById.values()].sort((left, right) =>
                    left.id.localeCompare(right.id),
                ),
                versions: {
                    ...result.versions,
                    proof,
                    daemon: snapshot.authenticatedPeer.daemonVer,
                    context: moduleVersion("context"),
                    synapse: moduleVersion("synapse"),
                    broca: moduleVersion("broca"),
                },
            },
        };
    }

    /**
     * Restamp a native result with the caller-facing command, first proving the
     * child answered the command this call is willing to accept.
     *
     * `parseDaemonResult` validates the restart-only `effects` invariant
     * against the child's own `command`, so blindly overwriting that field can
     * publish a `restart` result carrying `effects` under a `stop` or `start`
     * label. A disagreement means the child answered a different command than
     * requested — a real version-skew signal — so it becomes `internal_error`
     * rather than being silently relabeled.
     *
     * `expected` is the command the child must *report*, which is not always the
     * argv it was *sent*. Observational commands send the `probe` argv, but the
     * contract's command union is exactly start/stop/restart/status/doctor, so
     * the binary answers the read-only observation as `status` and a `probe`
     * response would be rejected by every contract-validating consumer.
     */
    private relabel(
        native: DaemonResultV1,
        expected: DaemonCommand,
        command: LifecycleCommand,
    ): DaemonResultV1 {
        if (native.command !== expected) {
            return localResult(command, false, "wedged", "internal_error", false);
        }
        return { ...native, command };
    }

    private launchFailure(command: LifecycleCommand, root: string, error: unknown): DaemonResultV1 {
        const state = preNativeState(classifyPreNativeRoots(root));
        // A launcher that already reduced the failure to a closed lifecycle
        // reason speaks for itself; re-deriving one from the error code would
        // discard the more specific classification it made.
        if (
            error !== null &&
            typeof error === "object" &&
            "reason" in error &&
            [
                "unsupported_platform",
                "unsupported_install_layout",
                "native_payload_missing",
                "native_payload_invalid",
                "insufficient_storage",
                "internal_error",
            ].includes(String((error as { reason?: unknown }).reason))
        ) {
            return localResult(
                command,
                false,
                state,
                (error as { reason: LifecycleFailureReason }).reason,
            );
        }
        if (error instanceof NativeLaunchError) {
            // If the native child ran, its effects are unknown; otherwise it committed nothing.
            const effectsKnown = !nativeChildRan(error);
            switch (error.code) {
                case "timeout":
                    return timeoutResult(command, root, effectsKnown);
                case "unsupported_platform":
                    return localResult(command, false, state, "unsupported_platform");
                default:
                    return localResult(command, false, state, "internal_error", effectsKnown);
            }
        }
        return localResult(command, false, state, "internal_error");
    }
}
