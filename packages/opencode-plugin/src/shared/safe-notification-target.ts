import { HOST_SDK_READ_TIMEOUT_MS, TimeoutError, withTimeout } from "./with-timeout";

/**
 * Post ignored notifications only to sessions with non-default titles.
 *
 * OpenCode permanently skips title generation after a session has more than one non-synthetic user message.
 * Ignored notifications remain non-synthetic user messages.
 * Posting an ignored notification before the first prompt can permanently suppress title generation.
 *
 * Do not mark notifications `synthetic: true`: Desktop renders only non-synthetic text parts.
 *
 * Posting to a session with a non-default title cannot affect title generation.
 */

/**
 * Keep this regex aligned with OpenCode's `Session.isDefaultTitle`.
 */
const DEFAULT_TITLE_RE =
    /^(New session - |Child session - )\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

export function isDefaultSessionTitle(title: string): boolean {
    return DEFAULT_TITLE_RE.test(title);
}

/**
 */
async function readSessionTitle(
    client: unknown,
    sessionId: string,
    timeoutMs: number,
): Promise<string | null | undefined> {
    try {
        const c = client as {
            session?: { get?: (input: unknown) => unknown };
        };
        if (typeof c.session?.get !== "function") return null;
        const raw = await withTimeout(
            Promise.resolve(c.session.get({ path: { id: sessionId } })),
            timeoutMs,
            "session title read timed out",
        );
        const obj = raw as { data?: { title?: unknown }; title?: unknown } | null;
        const title = obj && typeof obj === "object" ? (obj.data?.title ?? obj.title) : undefined;
        return typeof title === "string" ? title : null;
    } catch (error) {
        if (error instanceof TimeoutError) return undefined;
        return null;
    }
}

export interface SafeTargetOptions {
    /* */
    attempts?: number;
    /* */
    delayMs?: number;
    /* */
    readTimeoutMs?: number;
}

/**
 *
 *   unreadable (fail-open).
 * On `"skip"`, posting can permanently suppress the session's title generation.
 * The caller either drops an ordinary notice or retains a command result for a later retry.
 *
 */
export async function waitForSafeNotificationTarget(
    client: unknown,
    sessionId: string,
    options?: SafeTargetOptions,
): Promise<"safe" | "skip"> {
    const attempts = Math.max(1, options?.attempts ?? 4);
    const delayMs = options?.delayMs ?? 15_000;
    const readTimeoutMs = options?.readTimeoutMs ?? HOST_SDK_READ_TIMEOUT_MS;
    for (let attempt = 0; attempt < attempts; attempt += 1) {
        const title = await readSessionTitle(client, sessionId, readTimeoutMs);
        if (title === null) return "safe";
        if (title === undefined) return "skip";
        if (!isDefaultSessionTitle(title)) return "safe";
        if (attempt < attempts - 1) {
            await new Promise((resolve) => setTimeout(resolve, delayMs));
        }
    }
    return "skip";
}
