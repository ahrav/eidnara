import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
    type AuthenticatedPeer,
    BROCA_CREDENTIAL_NAMES,
    type CatalogEntry,
    HostClient,
    type HostClientOptions,
    type HostStatusSnapshot,
    sameDaemonId,
} from "../host-client";
import { BootstrapError, checkPlatform, type PlatformReaders } from "./bootstrap";
import {
    evaluateDaemonCompatibility,
    evaluateModuleCompatibility,
    observedEpochsFromContextMetrics,
} from "./compatibility";
import type { NativeStartupEnvelope } from "./native-launcher";
import { prepareManagedLaunchTarget, resolveManagedPayloadDir } from "./owner";
import { admitLifecycleFilesystem, connectionFilePath, resolveLifecycleDataRoot } from "./paths";
import {
    type CompatibilitySnapshot,
    HostLifecyclePolicy,
    type LifecyclePolicyOptions,
    monotonicNow,
    type ObservationalHealth,
    ReadinessProbeControlError,
    STORAGE_HARD_BUDGET_MS,
} from "./policy";

const MAX_PARENT_WALK = 8;
const READINESS_POLL_MS = 50;

export function buildManagedCredentialEnvelope(
    env: Record<string, string | undefined>,
): NativeStartupEnvelope {
    const credentials = Object.fromEntries(
        BROCA_CREDENTIAL_NAMES.flatMap((name) => {
            const value = env[name];
            return value === undefined || value.length === 0 ? [] : [[name, value]];
        }),
    );
    return {
        schema: 1,
        ...(Object.keys(credentials).length === 0 ? {} : { credentials }),
    };
}

function asRecord(value: unknown): Record<string, unknown> | null {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, unknown>)
        : null;
}

/** One `host.status` component record, or `null` when unavailable or malformed. */
function componentRecord(
    metrics: Record<string, unknown>,
    component: string,
): Record<string, unknown> | null {
    return asRecord(asRecord(metrics.components)?.[component]);
}

/** The `metrics` object of one `host.status` component, or `null` when absent. */
function componentMetrics(
    metrics: Record<string, unknown>,
    component: string,
): Record<string, unknown> | null {
    return asRecord(componentRecord(metrics, component)?.metrics);
}

function storageState(metrics: Record<string, unknown>): "ready" | "starting" | "unavailable" {
    const state = componentMetrics(metrics, "context")?.storage_state;
    return state === "ready" || state === "unavailable" ? state : "starting";
}

export type SynapseReadiness =
    | { state: "ready"; reason: "healthy" }
    | { state: "starting"; reason: "synapse_starting" }
    | { state: "degraded"; reason: "synapse_degraded" }
    | { state: "unsupported"; reason: "synapse_unsupported" };

export function synapseReadiness(metrics: Record<string, unknown>): SynapseReadiness {
    const component = componentRecord(metrics, "synapse");
    const state = asRecord(component?.metrics)?.synapse_state;
    if (state === "ready") return { state: "ready", reason: "healthy" };
    if (state === "starting") return { state: "starting", reason: "synapse_starting" };
    if (state === "unsupported") return { state: "unsupported", reason: "synapse_unsupported" };
    // The fixed profile always carries a Synapse lane and reports `unsupported` as an explicit literal, so an absent component, like a named one with a missing or out-of-set state, is a lane that cannot prove readiness and fails rather than reading as absent or unsupported.
    return { state: "degraded", reason: "synapse_degraded" };
}

export type KernelReadiness =
    | {
          state: "ready";
          reason: "healthy" | "kernel_lagging" | "kernel_capacity_warn" | "no_required_consumer";
      }
    | { state: "starting"; reason: "kernel_starting" }
    | { state: "unavailable"; reason: "kernel_unavailable" };

export function kernelReadiness(metrics: Record<string, unknown>): KernelReadiness {
    const kernel = asRecord(componentMetrics(metrics, "context")?.kernel);
    const state = kernel?.kernel_state;
    if (state === "starting") return { state: "starting", reason: "kernel_starting" };
    // An absent block is an unknown state and never reads as healthy.
    if (state !== "ready") return { state: "unavailable", reason: "kernel_unavailable" };
    // Warn reasons use priority order: lagging, capacity, then no required consumer.
    if (kernel?.lag_threshold_tripped === true) {
        return { state: "ready", reason: "kernel_lagging" };
    }
    if (kernel?.core_file_warn === true || kernel?.artifact_warn === true) {
        return { state: "ready", reason: "kernel_capacity_warn" };
    }
    if (kernel?.required_consumer_count === 0) {
        return { state: "ready", reason: "no_required_consumer" };
    }
    return { state: "ready", reason: "healthy" };
}

/**
 */
class StorageProbeDaemonMismatchError extends Error {
    constructor() {
        super("storage probe observed a different daemon than compatibility certified");
        this.name = "StorageProbeDaemonMismatchError";
    }
}

function assertStorageProbePeer(
    client: HostClient,
    expectedDaemonId: Uint8Array | undefined,
): void {
    if (expectedDaemonId === undefined) return;
    const daemonId = client.authenticated?.daemonId ?? null;
    if (daemonId === null || !sameDaemonId(daemonId, expectedDaemonId)) {
        throw new StorageProbeDaemonMismatchError();
    }
}

/**
 *
 */
async function probeManagedStorage(
    root: string,
    budgetMs: number,
    expectedDaemonId?: Uint8Array,
    signal?: AbortSignal,
): Promise<"ready" | "starting" | "unavailable"> {
    const deadline = monotonicNow() + budgetMs;
    const options: HostClientOptions = {
        connectionFile: connectionFilePath(root),
        handshakeTimeoutMs: Math.max(1, budgetMs),
        requestTimeoutMs: Math.max(1, budgetMs),
    };
    // A cached client is shared; closing it would disconnect a concurrent probe for the same root and budget, which would report a healthy daemon as unavailable.
    let client: HostClient | undefined;
    try {
        client = await HostClient.connect(options);
        assertStorageProbePeer(client, expectedDaemonId);
        for (;;) {
            const snapshot = await client.hostStatus({
                timeoutMs: Math.max(1, deadline - monotonicNow()),
            });
            assertStorageProbePeer(client, expectedDaemonId);
            const state = storageState(snapshot.metrics);
            // An abort means no waiter remains; the observation is left indeterminate rather than polled to the deadline.
            if (state !== "starting" || monotonicNow() >= deadline || signal?.aborted) return state;
            await new Promise<void>((resolve) => {
                const onAbort = (): void => {
                    clearTimeout(timer);
                    resolve();
                };
                const timer = setTimeout(
                    () => {
                        // `once` removes the listener only when abort fires; the timer path must remove it too or every poll iteration leaves one behind.
                        signal?.removeEventListener("abort", onAbort);
                        resolve();
                    },
                    Math.min(READINESS_POLL_MS, Math.max(1, deadline - monotonicNow())),
                );
                signal?.addEventListener("abort", onAbort, { once: true });
            });
            if (signal?.aborted) return "starting";
        }
    } catch (error) {
        if (error instanceof StorageProbeDaemonMismatchError) throw error;
        return monotonicNow() >= deadline ? "starting" : "unavailable";
    } finally {
        // The connected channel holds a referenced interval, so a one-shot caller stays alive until this client closes.
        // Teardown runs under `closeAsync`'s own shutdown deadline and is not awaited, so a settled state reaches the policy inside its storage budget.
        if (client !== undefined) void client.closeAsync().catch(() => undefined);
    }
}

export interface ManagedCompatibilityClient {
    readonly authenticated: AuthenticatedPeer | null;
    catalogList(options?: { timeoutMs?: number }): Promise<CatalogEntry[]>;
    hostStatus(options?: { timeoutMs?: number }): Promise<HostStatusSnapshot>;
}

function samePeer(left: AuthenticatedPeer | null, right: AuthenticatedPeer): boolean {
    if (left === null || left.daemonVer !== right.daemonVer || left.proof !== right.proof) {
        return false;
    }
    return sameDaemonId(left.daemonId, right.daemonId);
}

export interface CompatibilityProbeResult {
    snapshot: CompatibilitySnapshot;
    status: HostStatusSnapshot | null;
}

/**
 * `deadline` is a `monotonicNow()` timestamp.
 */
async function readCompatibilityProbe(
    client: ManagedCompatibilityClient,
    deadline: number,
    signal?: AbortSignal,
): Promise<CompatibilityProbeResult> {
    const authenticated = client.authenticated;
    if (authenticated === null || authenticated.daemonId === null) {
        throw new Error("authenticated peer disappeared");
    }
    const daemon = evaluateDaemonCompatibility(authenticated);
    if (!daemon.ok) {
        return {
            snapshot: {
                authenticatedPeer: {
                    ...authenticated,
                    daemonId: Uint8Array.from(authenticated.daemonId),
                },
                authenticatedDaemonVersion: authenticated.daemonVer,
                authenticatedDaemonId: Uint8Array.from(authenticated.daemonId),
                catalog: [],
                epochs: {},
                evaluatedThrough: "daemon",
            },
            status: null,
        };
    }
    const catalogMs = deadline - monotonicNow();
    if (catalogMs <= 0) throw new Error("compatibility probe deadline expired");
    const catalog = await client.catalogList({ timeoutMs: catalogMs });
    if (!samePeer(client.authenticated, authenticated)) {
        throw new Error("authenticated peer changed during compatibility probe");
    }
    if (signal?.aborted) throw signal.reason ?? new Error("compatibility probe aborted");
    const modules = evaluateModuleCompatibility(catalog);
    if (!modules.ok) {
        return {
            snapshot: {
                authenticatedPeer: {
                    ...authenticated,
                    daemonId: Uint8Array.from(authenticated.daemonId),
                },
                authenticatedDaemonVersion: authenticated.daemonVer,
                authenticatedDaemonId: Uint8Array.from(authenticated.daemonId),
                catalog,
                epochs: {},
                evaluatedThrough: "modules",
            },
            status: null,
        };
    }
    const remainingMs = deadline - monotonicNow();
    if (remainingMs <= 0) throw new Error("compatibility probe deadline expired");
    const status = await client.hostStatus({
        timeoutMs: remainingMs,
    });
    if (!samePeer(client.authenticated, authenticated)) {
        throw new Error("authenticated peer changed during compatibility probe");
    }
    if (signal?.aborted) throw signal.reason ?? new Error("compatibility probe aborted");
    const contextMetrics = componentMetrics(status.metrics, "context");
    // The probe reports observations, not a compatibility verdict.
    const snapshot = {
        authenticatedPeer: {
            ...authenticated,
            daemonId: Uint8Array.from(authenticated.daemonId),
        },
        authenticatedDaemonVersion: authenticated.daemonVer,
        authenticatedDaemonId: Uint8Array.from(authenticated.daemonId),
        catalog,
        epochs: observedEpochsFromContextMetrics(contextMetrics),
        evaluatedThrough: "epochs" as const,
    };
    return { snapshot, status };
}

/** `deadline` is a `monotonicNow()` timestamp. */
export async function readCompatibilitySnapshot(
    client: ManagedCompatibilityClient,
    deadline: number,
    signal?: AbortSignal,
): Promise<CompatibilitySnapshot> {
    return (await readCompatibilityProbe(client, deadline, signal)).snapshot;
}

async function probeManagedCompatibility(
    root: string,
    budgetMs: number,
    signal?: AbortSignal,
): Promise<CompatibilityProbeResult> {
    const deadline = monotonicNow() + budgetMs;
    const client = await HostClient.connect({
        connectionFile: connectionFilePath(root),
        handshakeTimeoutMs: Math.max(1, budgetMs),
        requestTimeoutMs: Math.max(1, budgetMs),
    });
    try {
        return await readCompatibilityProbe(client, deadline, signal);
    } finally {
        // Teardown is not awaited: the policy shares this probe across demands and a caller without its own deadline waits for the promise to settle, so a slow Goodbye flush would hold a completed observation for a second aggregate.
        void client.closeAsync().catch(() => undefined);
    }
}

async function probeManagedReadiness(root: string, budgetMs: number): Promise<ObservationalHealth> {
    const deadline = monotonicNow() + budgetMs;
    // A private client, like the storage probe's. The residual budget varies per
    // call and `ownerKey` includes the timeouts, so a shared owner would cache a
    // new client — and prefault another ring — on every status or doctor, until
    // admission is exhausted.
    const client = await HostClient.connect({
        connectionFile: connectionFilePath(root),
        handshakeTimeoutMs: Math.max(1, budgetMs),
        requestTimeoutMs: Math.max(1, budgetMs),
    });
    let probe: CompatibilityProbeResult;
    try {
        probe = await readCompatibilityProbe(client, deadline);
    } catch (error) {
        if (client.isClosed || client.authenticated === null) throw error;
        throw new ReadinessProbeControlError(error);
    } finally {
        // Teardown is not part of the observation, and `closeAsync` opens its own
        // shutdown deadline, so awaiting it here could settle this promise after
        // the lifecycle command's aggregate expired.
        void client.closeAsync().catch(() => undefined);
    }
    const { snapshot: compatibility, status } = probe;
    if (status === null) {
        // The probe short-circuited at the daemon or module stage, so
        // `host.status` never ran and storage and Synapse were never
        // observed. Report only what the handshake proved and leave the
        // unobserved components absent rather than asserting failures that
        // would point remediation away from the version mismatch.
        return {
            ...compatibility,
            readiness: { transport: { state: "ready", reason: "healthy" } },
        };
    }
    const storage = storageState(status.metrics);
    const kernel = kernelReadiness(status.metrics);
    const synapse = synapseReadiness(status.metrics);
    return {
        ...compatibility,
        readiness: {
            transport: { state: "ready", reason: "healthy" },
            storage: {
                state: storage,
                reason:
                    storage === "ready"
                        ? "healthy"
                        : storage === "starting"
                          ? "storage_starting"
                          : "storage_unavailable",
            },
            synapse,
            kernel,
        },
    };
}

export interface ManagedLifecyclePolicyOptions
    extends Omit<LifecyclePolicyOptions, "launchTarget" | "payloadDir" | "bootstrapFailure"> {
    mode: "mutating" | "observational";
    declaringModuleUrl: string;
    parentPackageName: string;
    explicitExternalRoot?: string;
}

type StorageReadinessState = "ready" | "starting" | "unavailable";

export interface ManagedProbeIo {
    compatibility(budgetMs: number, signal?: AbortSignal): Promise<CompatibilityProbeResult>;
    storage(
        budgetMs: number,
        expectedDaemonId?: Uint8Array,
        signal?: AbortSignal,
    ): Promise<StorageReadinessState>;
}

export interface ManagedProbes {
    compatibilityProbe(budgetMs: number, signal?: AbortSignal): Promise<CompatibilitySnapshot>;
    storageProbe(
        budgetMs: number,
        expectedDaemonId?: Uint8Array,
        signal?: AbortSignal,
    ): Promise<StorageReadinessState>;
}

function daemonKey(daemonId: Uint8Array): string {
    return Buffer.from(daemonId).toString("hex");
}

interface SharedStoragePoll {
    result: Promise<StorageReadinessState>;
    controller: AbortController;
    waiters: number;
}

/**
 * A waiter that outlives its own budget answers `starting`, the state a private poll of that length would have returned; the shared poll keeps running for the waiters still entitled to wait. `onIdle` fires when the last waiter leaves a poll that has not settled.
 *
 * A waiter whose caller aborts leaves at once, so a canceled demand stops holding the shared poll (and its connection) open for the rest of its budget.
 */
function joinStoragePoll(
    poll: SharedStoragePoll,
    budgetMs: number,
    onIdle: () => void,
    signal?: AbortSignal,
): Promise<StorageReadinessState> {
    poll.waiters += 1;
    return new Promise((resolve, reject) => {
        let settled = false;
        let timer: ReturnType<typeof setTimeout> | null = null;
        const onAbort = (): void => {
            leave();
            resolve("starting");
        };
        const leave = (): void => {
            if (settled) return;
            settled = true;
            if (timer !== null) clearTimeout(timer);
            signal?.removeEventListener("abort", onAbort);
            poll.waiters -= 1;
            if (poll.waiters === 0) onIdle();
        };
        if (signal?.aborted) {
            onAbort();
            return;
        }
        signal?.addEventListener("abort", onAbort, { once: true });
        timer = setTimeout(
            () => {
                leave();
                resolve("starting");
            },
            Math.max(1, budgetMs),
        );
        poll.result.then(
            (state) => {
                leave();
                resolve(state);
            },
            (error: unknown) => {
                leave();
                reject(error);
            },
        );
    });
}

/**
 * The compatibility probe records the storage state with the reporting daemon ID. A storage probe expecting that daemon answers a terminal `ready` or `unavailable` from the record, so readiness and compatibility describe one observation and the storage probe opens no connection of its own; a `starting` record still polls within the storage budget.
 *
 * The record is not consumed on read. The policy shares one compatibility probe across concurrent demands and each of them runs its own storage probe, so a one-shot slot would hand the observation to the first waiter and send every other waiter to open a connection. Each later compatibility probe replaces the record, and a storage probe always follows the compatibility probe of its own demand, so no demand reads a record older than its own observation.
 *
 * Polls for the same daemon coalesce for the same reason: each connection attaches and prefaults a shared-memory ring, so a burst of demands during startup would otherwise spend admission on redundant probes. The shared poll runs on the hard storage budget rather than the first waiter's remaining budget, so a nearly expired waiter arriving first cannot mint a poll too short for the waiters that join it; each waiter bounds its own wait, and the poll is aborted once no waiter remains.
 */
export function managedProbes(io: ManagedProbeIo): ManagedProbes {
    let observed: { daemonId: Uint8Array; state: StorageReadinessState } | null = null;
    let issued = 0;
    const polling = new Map<string, SharedStoragePoll>();
    return {
        async compatibilityProbe(budgetMs, signal) {
            const sequence = ++issued;
            const probe = await io.compatibility(budgetMs, signal);
            // Only the most recently issued probe may write the record. A probe the policy already gave up on can still settle after its replacement, and its older storage state must not displace the newer one.
            if (sequence === issued) {
                observed =
                    probe.status === null
                        ? null
                        : {
                              daemonId: Uint8Array.from(probe.snapshot.authenticatedPeer.daemonId),
                              state: storageState(probe.status.metrics),
                          };
            }
            return probe.snapshot;
        },
        storageProbe(budgetMs, expectedDaemonId, signal) {
            if (expectedDaemonId === undefined) return io.storage(budgetMs);
            const record = observed;
            if (
                record !== null &&
                sameDaemonId(record.daemonId, expectedDaemonId) &&
                record.state !== "starting"
            ) {
                return Promise.resolve(record.state);
            }
            const key = daemonKey(expectedDaemonId);
            let poll = polling.get(key);
            if (poll === undefined) {
                const controller = new AbortController();
                const created: SharedStoragePoll = {
                    result: io.storage(STORAGE_HARD_BUDGET_MS, expectedDaemonId, controller.signal),
                    controller,
                    waiters: 0,
                };
                poll = created;
                polling.set(key, created);
                const evict = (): void => {
                    if (polling.get(key) === created) polling.delete(key);
                };
                void created.result.then(evict, evict);
            }
            const current = poll;
            return joinStoragePoll(
                current,
                budgetMs,
                () => {
                    current.controller.abort();
                    if (polling.get(key) === current) polling.delete(key);
                },
                signal,
            );
        },
    };
}

function findDeclaringParentRoot(moduleUrl: string, packageName: string): string {
    let current = dirname(fileURLToPath(moduleUrl));
    for (let depth = 0; depth <= MAX_PARENT_WALK; depth += 1) {
        const packagePath = join(current, "package.json");
        let text: string | null;
        try {
            text = readFileSync(packagePath, "utf8");
        } catch (error) {
            const code = (error as NodeJS.ErrnoException).code;
            // Only a genuine absence lets the walk climb. A descriptor that exists but cannot be read may name this package, and climbing past it would certify a farther install's payload as the declaring one.
            if (code !== "ENOENT" && code !== "ENOTDIR") {
                throw new BootstrapError(
                    "unsupported_install_layout",
                    "declaring parent package descriptor is not inspectable",
                );
            }
            text = null;
        }
        if (text !== null) {
            const parsed = asRecord(parseJsonOrNull(text));
            if (parsed === null) {
                throw new BootstrapError(
                    "unsupported_install_layout",
                    "declaring parent package descriptor is malformed",
                );
            }
            if (parsed.name === packageName) return current;
        }
        const parent = dirname(current);
        if (parent === current) break;
        current = parent;
    }
    throw new BootstrapError(
        "unsupported_install_layout",
        "declaring parent package root is unavailable",
    );
}

function parseJsonOrNull(text: string): unknown {
    try {
        return JSON.parse(text) as unknown;
    } catch {
        return null;
    }
}

/**
 */
export function createManagedLifecyclePolicy(
    options: ManagedLifecyclePolicyOptions,
): HostLifecyclePolicy {
    // Construction admits the data root, stages the bootstrap under it, and pins the probes to it; the policy's per-command root resolution reads the same snapshot so a later mutation of the supplied object cannot send commands to one root and probes to another.
    const env: Record<string, string | undefined> = { ...(options.env ?? process.env) };
    const root = resolveLifecycleDataRoot(env);
    if (!root.ok) return new HostLifecyclePolicy({ ...options, env });

    const readers: PlatformReaders | undefined = options.platformReaders;
    const platform = checkPlatform(readers);
    if (!platform.ok) return new HostLifecyclePolicy({ ...options, env });

    // Admission precedes preparation because preparation writes to the data root.
    // A pre-native rejection must leave no trace.
    //
    // `preflight()` is the sole authority for the admission outcome.
    if (!admitLifecycleFilesystem(root.root, options.admissionIo).ok) {
        return new HostLifecyclePolicy({ ...options, env });
    }

    try {
        // A compiled Bun caller declares its module from an embedded filesystem with no physical package tree, so the parent walk would fail before the external root is examined; the payload resolver never reads the lexical root once an explicit root is supplied.
        const payloadLocator =
            options.explicitExternalRoot === undefined
                ? {
                      declaringParentRoot: findDeclaringParentRoot(
                          options.declaringModuleUrl,
                          options.parentPackageName,
                      ),
                  }
                : { explicitExternalRoot: options.explicitExternalRoot };
        const prepared = prepareManagedLaunchTarget({
            dataRoot: root.root,
            target: platform.target,
            allowStaging: options.mode === "mutating",
            ...payloadLocator,
        });
        const probes = managedProbes({
            compatibility: (budgetMs, signal) =>
                probeManagedCompatibility(root.root, budgetMs, signal),
            storage: (budgetMs, expectedDaemonId, signal) =>
                probeManagedStorage(root.root, budgetMs, expectedDaemonId, signal),
        });
        return new HostLifecyclePolicy({
            ...options,
            env,
            launchTarget: prepared,
            defaultStartupEnvelope: buildManagedCredentialEnvelope(env),
            storageProbe: options.storageProbe ?? probes.storageProbe,
            compatibilityProbe: options.compatibilityProbe ?? probes.compatibilityProbe,
            readinessProbe:
                options.readinessProbe ??
                ((budgetMs) => probeManagedReadiness(root.root, budgetMs)),
            ...(prepared?.payloadDir === undefined ? {} : { payloadDir: prepared.payloadDir }),
            ...(prepared === null ? {} : { payloadManifestDigest: prepared.payloadManifestDigest }),
            ...(options.mode === "mutating" && prepared?.payloadDir === undefined
                ? {
                      payloadDirFallback: () =>
                          resolveManagedPayloadDir({
                              target: platform.target,
                              ...payloadLocator,
                          }),
                  }
                : {}),
        });
    } catch (error) {
        return new HostLifecyclePolicy({
            ...options,
            env,
            launchTarget: null,
            bootstrapFailure: error instanceof BootstrapError ? error.reason : "internal_error",
        });
    }
}
