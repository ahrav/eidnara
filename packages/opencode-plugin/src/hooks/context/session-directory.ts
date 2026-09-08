import type { PluginContext } from "../../plugin/types";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "../../shared/with-timeout";
import { recordChildSession } from "./live-session-state";

export interface SessionDirectoryDeps {
    client?: PluginContext["client"];
    /** The plugin launch directory; the route root when the host reports none for the session. */
    directory?: string;
    /** Caches each session's first resolved route root so later daemon calls use the same root. */
    sessionDirectoryBySession?: Map<string, string>;
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
 * cannot change the daemon route. commentlint: allow(JUDGE)
 */
export async function resolveSessionDirectory(
    deps: SessionDirectoryDeps,
    sessionId: string,
): Promise<string> {
    const pinned = deps.sessionDirectoryBySession?.get(sessionId);
    if (pinned) return pinned;
    const fallback = knownSessionDirectory(deps, sessionId);
    if (!deps.client?.session?.get) return pin(deps, sessionId, fallback);
    try {
        const response = await withTimeout(
            deps.client.session.get({ path: { id: sessionId } }),
            HOST_SDK_READ_TIMEOUT_MS,
            "session directory read timed out",
        );
        const session = (response as { data?: unknown } | null)?.data as
            | { directory?: unknown; parentID?: unknown; title?: unknown }
            | undefined;
        if (session) recordChildSession(deps, sessionId, session);
        const directory = session?.directory;
        if (typeof directory === "string" && directory.length > 0) {
            return pin(deps, sessionId, directory);
        }
    } catch {
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
