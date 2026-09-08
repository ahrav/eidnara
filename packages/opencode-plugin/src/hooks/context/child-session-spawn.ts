import { normalizeSDKResponse } from "../../shared/normalize-sdk-response";
import { withTimeout } from "../../shared/with-timeout";

/** Child-session create and delete only touch OpenCode's session table; a call still pending after this long is a stuck endpoint, and the command awaiting it must not hang with it. */
const CHILD_SESSION_LIFECYCLE_TIMEOUT_MS = 10_000;
let childSessionLifecycleTimeoutMs = CHILD_SESSION_LIFECYCLE_TIMEOUT_MS;

export const __childSessionSpawnTest = {
    setLifecycleTimeoutMs(timeoutMs: number): void {
        childSessionLifecycleTimeoutMs = timeoutMs;
    },
    reset(): void {
        childSessionLifecycleTimeoutMs = CHILD_SESSION_LIFECYCLE_TIMEOUT_MS;
    },
};

interface ChildSessionClient {
    session: { create(input: never): unknown | Promise<unknown> };
}

interface ChildSessionDeleteClient {
    session: { delete(input: never): unknown | Promise<unknown> };
}

interface ChildSessionSpawnArgs {
    client: ChildSessionClient;
    parentSessionId?: string;
    title: string;
    directory?: string;
}

export async function createChildSession(args: ChildSessionSpawnArgs): Promise<unknown> {
    return withTimeout(
        Promise.resolve(
            args.client.session.create({
                body: {
                    ...(args.parentSessionId ? { parentID: args.parentSessionId } : {}),
                    title: args.title,
                },
                query: { directory: args.directory },
            } as never),
        ),
        childSessionLifecycleTimeoutMs,
        "child session create timed out",
    );
}

/** Deletion is best-effort cleanup: the caller has already finished with the child, so a failed or slow delete is logged by the caller and never blocks it past the deadline. */
export async function deleteChildSession(
    client: ChildSessionDeleteClient,
    sessionId: string,
): Promise<void> {
    await withTimeout(
        Promise.resolve(client.session.delete({ path: { id: sessionId } } as never)),
        childSessionLifecycleTimeoutMs,
        "child session delete timed out",
    );
}

interface ChildSessionMessagesClient {
    session: { messages(input: never): unknown | Promise<unknown> };
}

/**
 * Builds the `fetchOutput` closure a child-session prompt run hands to
 * `promptSyncWithValidatedOutputRetry`: read the child session's transcript and
 * normalize the SDK envelope, preferring response data over a bare wrapper.
 * The session id is bound at construction; callers create the child session first.
 */
export function childSessionMessagesFetcher(
    client: ChildSessionMessagesClient,
    sessionId: string,
    directory: string | undefined,
    limit: number,
): () => Promise<unknown[]> {
    return async () => {
        const messagesResponse = await client.session.messages({
            path: { id: sessionId },
            query: { directory, limit },
        } as never);
        return normalizeSDKResponse(messagesResponse, [] as unknown[], {
            preferResponseOnMissingData: true,
        });
    };
}
