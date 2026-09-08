import type { PluginContext } from "../../plugin/types";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "../../shared/with-timeout";
import { recordChildSession, type SessionMetadataReadState } from "./live-session-state";

const MAX_SESSION_METADATA_READ_ATTEMPTS = 2;
const SESSION_METADATA_RETRY_DELAY_MS = 1_000;
let sessionMetadataRetryDelayMs = SESSION_METADATA_RETRY_DELAY_MS;

export const __sessionDirectoryTest = {
    setRetryDelayMs(delayMs: number): void {
        sessionMetadataRetryDelayMs = delayMs;
    },
    reset(): void {
        sessionMetadataRetryDelayMs = SESSION_METADATA_RETRY_DELAY_MS;
    },
};

export interface SessionDirectoryDeps {
    client?: PluginContext["client"];
    /** The plugin launch directory; the route root when the host reports none for the session. */
    directory?: string;
    /** Caches each session's first resolved route root so later daemon calls use the same root. */
    sessionDirectoryBySession?: Map<string, string>;
    /** The maximum attempt count denotes a completed read; failures carry the earliest retry time. commentlint: allow(JUDGE) */
    sessionMetadataReadStateBySession?: Map<string, SessionMetadataReadState>;
    /** Tracks sessions whose directory response has a non-empty string `parentID`. */
    subagentSessions?: Set<string>;
    /** Tracks child sessions whose directory response carries the internal `eidnara-` title prefix. */
    internalChildSessions?: Set<string>;
}

export function knownSessionDirectory(deps: SessionDirectoryDeps, sessionId: string): string {
    return deps.sessionDirectoryBySession?.get(sessionId) ?? deps.directory ?? process.cwd();
}

/**
 * The daemon keys session state by `(session, project_root)`, so all calls for one session use one
 * directory. A missing, failed, or slow read pins the fallback directory so later successful reads
 * cannot change the daemon route. Child classification is independent of routing and gets one
 * retry after a failed first read. commentlint: allow(JUDGE)
 */
export async function resolveSessionDirectory(
    deps: SessionDirectoryDeps,
    sessionId: string,
): Promise<string> {
    const pinned = deps.sessionDirectoryBySession?.get(sessionId);
    const metadataStates = deps.sessionMetadataReadStateBySession;
    const metadataState = metadataStates?.get(sessionId);
    if (
        pinned &&
        metadataState?.inFlight === undefined &&
        (metadataStates === undefined ||
            (metadataState !== undefined &&
                (metadataState.attempts >= MAX_SESSION_METADATA_READ_ATTEMPTS ||
                    Date.now() < metadataState.retryAfterMs)))
    )
        return pinned;
    const fallback = pinned ?? knownSessionDirectory(deps, sessionId);
    if (!deps.client?.session?.get) return pin(deps, sessionId, fallback);
    const attempts = metadataState?.inFlight
        ? metadataState.attempts
        : (metadataState?.attempts ?? 0) + 1;
    const inFlight =
        metadataState?.inFlight ??
        withTimeout(
            deps.client.session.get({ path: { id: sessionId } }),
            HOST_SDK_READ_TIMEOUT_MS,
            "session directory read timed out",
        ).then((response) => {
            const value = (response as { data?: unknown } | null)?.data;
            return value && typeof value === "object"
                ? (value as {
                      directory?: unknown;
                      parentID?: unknown;
                      title?: unknown;
                  })
                : null;
        });
    if (metadataState?.inFlight === undefined) {
        metadataStates?.set(sessionId, {
            attempts,
            retryAfterMs: Number.POSITIVE_INFINITY,
            inFlight,
        });
    }
    try {
        const session = await inFlight;
        if (session) {
            recordChildSession(deps, sessionId, session);
            metadataStates?.set(sessionId, {
                attempts: MAX_SESSION_METADATA_READ_ATTEMPTS,
                retryAfterMs: Number.POSITIVE_INFINITY,
            });
        } else {
            metadataStates?.set(sessionId, {
                attempts,
                retryAfterMs: Date.now() + sessionMetadataRetryDelayMs,
            });
        }
        const directory = session?.directory;
        if (typeof directory === "string" && directory.length > 0) {
            return pin(deps, sessionId, directory);
        }
    } catch {
        metadataStates?.set(sessionId, {
            attempts,
            retryAfterMs: Date.now() + sessionMetadataRetryDelayMs,
        });
        // Routing falls back to the launch directory without failing the caller.
    }
    return pin(deps, sessionId, fallback);
}

/** The first resolution to reach `pin` wins; later resolutions return its cached directory. */
function pin(deps: SessionDirectoryDeps, sessionId: string, directory: string): string {
    const cache = deps.sessionDirectoryBySession;
    if (!cache) return directory;
    const installed = cache.get(sessionId);
    if (installed !== undefined) return installed;
    cache.set(sessionId, directory);
    return directory;
}
