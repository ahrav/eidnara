import type { PluginContext } from "../../plugin/types";
import { withTimeout } from "../../shared/with-timeout";

/** Limits OpenCode SDK reads so a slow host cannot hold a hook open indefinitely. */
export const HOST_SDK_READ_TIMEOUT_MS = 2_000;

export interface SessionDirectoryDeps {
    client?: PluginContext["client"];
    /** The plugin launch directory; the route root when the host reports none for the session. */
    directory?: string;
    /** Host-reported `session.directory` values, so one SDK read per session serves every later call. */
    sessionDirectoryBySession?: Map<string, string>;
}

/** The directory known without an SDK call: the cached host value, else the launch directory. */
export function knownSessionDirectory(deps: SessionDirectoryDeps, sessionId: string): string {
    return deps.sessionDirectoryBySession?.get(sessionId) ?? deps.directory ?? process.cwd();
}

/**
 * The daemon keys a session's state by `(session, project_root)`, so every daemon call for one
 * session must route by the same directory. The host's `session.directory` wins; a missing,
 * failed, or slow read falls back to the launch directory rather than failing the caller.
 */
export async function resolveSessionDirectory(
    deps: SessionDirectoryDeps,
    sessionId: string,
): Promise<string> {
    const cached = deps.sessionDirectoryBySession?.get(sessionId);
    if (cached) return cached;
    if (!deps.client?.session?.get) return knownSessionDirectory(deps, sessionId);
    try {
        const response = await withTimeout(
            deps.client.session.get({ path: { id: sessionId } }),
            HOST_SDK_READ_TIMEOUT_MS,
            "session directory read timed out",
        );
        const directory = (response as { data?: { directory?: unknown } } | null)?.data?.directory;
        if (typeof directory === "string" && directory.length > 0) {
            deps.sessionDirectoryBySession?.set(sessionId, directory);
            return directory;
        }
    } catch {
        // Routing falls back to the launch directory without failing the caller.
    }
    return knownSessionDirectory(deps, sessionId);
}
