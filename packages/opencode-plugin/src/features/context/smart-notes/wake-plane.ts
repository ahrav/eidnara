import { statSync } from "node:fs";
import { getDataDir } from "../../../shared/data-path";
import { HostClient } from "../../../shared/host-client";
import { defaultConnectionFilePath } from "../../../shared/host-lifecycle/paths";

/** `wake.create` indicates that scheduled wakes own condition evaluation. */
export const WAKE_PLANE_CAPABILITY = "wake.create";

export type WakePlaneStatus = "present" | "absent" | "unknown";

const WAKE_PLANE_STATUS_TTL_MS = 5 * 60 * 1_000;
const WAKE_PLANE_HANDSHAKE_TIMEOUT_MS = 2_000;
const WAKE_PLANE_CATALOG_TIMEOUT_MS = 2_000;

type CatalogEntry = { control_ops?: unknown };
type CatalogProbe = (connectionFile: string) => Promise<readonly CatalogEntry[]>;
type PublicationReader = (connectionFile: string) => string | null;

interface WakePlaneStatusCache {
    status: WakePlaneStatus;
    expiresAt: number;
    /** The connection file names the daemon the answer describes; a configured file and the default may differ. */
    connectionFile: string;
    /** The cache records the publication against which the answer was proved; `null` means none was readable. */
    publication: string | null;
}

interface InFlightProbe {
    probe: Promise<WakePlaneStatus>;
    connectionFile: string;
    publication: string | null;
}

let cachedStatus: WakePlaneStatusCache | null = null;
/** The in-flight probe records the daemon and publication it is bound to so only matching callers coalesce onto it. */
let inFlight: InFlightProbe | null = null;
let catalogProbe: CatalogProbe = probeWakePlaneCatalog;
let readPublication: PublicationReader = readDaemonPublication;
let now = () => Date.now();

function defaultConnectionFile(): string {
    return defaultConnectionFilePath(getDataDir());
}

/**
 * The publication fingerprint identifies the daemon publication against which the answer was proved.
 */
function readDaemonPublication(connectionFile: string): string | null {
    try {
        const stat = statSync(connectionFile);
        return `${stat.dev}:${stat.ino}:${stat.mtimeMs}:${stat.size}`;
    } catch {
        return null;
    }
}

async function probeWakePlaneCatalog(connectionFile: string): Promise<readonly CatalogEntry[]> {
    // Connect at probe time so the answer comes from the daemon that currently owns the
    // connection file; a cached client can outlive a replaced publication.
    const client = await HostClient.connect({
        connectionFile,
        handshakeTimeoutMs: WAKE_PLANE_HANDSHAKE_TIMEOUT_MS,
        credentialSource: process.env,
    });
    try {
        return await client.catalogList({ timeoutMs: WAKE_PLANE_CATALOG_TIMEOUT_MS });
    } finally {
        // Teardown is not part of the answer; `closeAsync` runs under its own shutdown deadline.
        void client.closeAsync().catch(() => undefined);
    }
}

function catalogHasWakePlane(entries: readonly CatalogEntry[]): boolean {
    return entries.some(
        (entry) =>
            Array.isArray(entry.control_ops) && entry.control_ops.includes(WAKE_PLANE_CAPABILITY),
    );
}

async function probeStatus(connectionFile: string): Promise<WakePlaneStatus> {
    try {
        return catalogHasWakePlane(await catalogProbe(connectionFile)) ? "present" : "absent";
    } catch {
        // Connection and catalog failures return `unknown` so standalone smart notes remain on.
        return "unknown";
    }
}

/**
 * Every retained answer is bound to the daemon publication it was proved
 * against. An affirmative answer may only
 * be reused while that daemon still owns the publication, so a replacement can
 * never inherit the capability. A negative or unknown answer is bound the same
 * way: under lazy demand-start the common case is a passive probe that runs
 * BEFORE the first Rust or Synapse demand, and the managed start that follows
 * publishes a new connection file. Without this binding that answer would keep
 * standalone evaluation on for the rest of its TTL while the daemon already
 * owns scheduled wakes, so both planes would evaluate the same conditions.
 */
function isRetainedAnswerUsable(cache: WakePlaneStatusCache, connectionFile: string): boolean {
    if (cache.connectionFile !== connectionFile) return false;
    // `present` with no readable publication is never reusable.
    if (cache.status === "present" && cache.publication === null) return false;
    return cache.publication === readPublication(connectionFile);
}

export interface WakePlaneStatusOptions {
    /** Connection file for the daemon whose catalog determines the result. */
    connectionFile?: string;
}

/**
 * The status probe determines whether scheduled wakes own condition evaluation.
 * Only an affirmative catalog capability disables standalone smart notes.
 * An unreachable daemon or a catalog without `wake.create` leaves standalone smart notes enabled.
 */
export async function wakePlaneStatus(
    options: WakePlaneStatusOptions = {},
): Promise<WakePlaneStatus> {
    const connectionFile = options.connectionFile ?? defaultConnectionFile();
    const cached = cachedStatus;
    if (cached && now() < cached.expiresAt && isRetainedAnswerUsable(cached, connectionFile)) {
        return cached.status;
    }

    // The pre-probe publication binds the result to the daemon observed before probing.
    const publication = readPublication(connectionFile);
    if (
        inFlight &&
        inFlight.connectionFile === connectionFile &&
        inFlight.publication === publication
    ) {
        return await inFlight.probe;
    }

    const startedAt = now();
    const probe = probeStatus(connectionFile).then((status) => {
        // Stale publications must be rejected before the probe settles so coalesced callers cannot receive stale results.
        // A stale result can describe a daemon that no longer serves requests.
        // After a restart, a stale result can report `present` for the old daemon.
        // A replacement daemon without `wake.create` makes an old `present` result stale.
        //
        // The function returns `unknown` because the result cannot be bound to a publication.
        if (readPublication(connectionFile) !== publication) {
            // A newer probe may already have cached an answer for the current daemon.
            if (
                cachedStatus?.connectionFile === connectionFile &&
                cachedStatus.publication === publication
            ) {
                cachedStatus = null;
            }
            return "unknown" as WakePlaneStatus;
        }
        // The cache does not retain `present` when `publication` is `null` because no daemon identity is available.
        cachedStatus =
            status === "present" && publication === null
                ? null
                : {
                      status,
                      expiresAt: startedAt + WAKE_PLANE_STATUS_TTL_MS,
                      connectionFile,
                      publication,
                  };
        return status;
    });
    inFlight = { probe, connectionFile, publication };
    try {
        return await probe;
    } finally {
        if (inFlight?.probe === probe) inFlight = null;
    }
}

export const __wakePlaneTest = {
    reset(): void {
        cachedStatus = null;
        inFlight = null;
        catalogProbe = probeWakePlaneCatalog;
        readPublication = readDaemonPublication;
        now = () => Date.now();
    },
    setCatalogProbe(probe: CatalogProbe): void {
        catalogProbe = probe;
    },
    setPublicationReader(reader: PublicationReader): void {
        readPublication = reader;
    },
    setNow(clock: () => number): void {
        now = clock;
    },
    connectionFile: defaultConnectionFile,
    ttlMs: WAKE_PLANE_STATUS_TTL_MS,
};
