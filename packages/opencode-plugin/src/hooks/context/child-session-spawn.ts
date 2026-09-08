import { normalizeSDKResponse } from "../../shared/normalize-sdk-response";
import { HOST_SDK_READ_TIMEOUT_MS, TimeoutError, withTimeout } from "../../shared/with-timeout";

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

interface ChildSessionDeleteClient {
    session: { delete(input: never): unknown | Promise<unknown> };
}

interface ChildSessionClient extends ChildSessionDeleteClient {
    session: ChildSessionDeleteClient["session"] & {
        create(input: never): unknown | Promise<unknown>;
    };
}

interface ChildSessionSpawnArgs {
    client: ChildSessionClient;
    parentSessionId?: string;
    title: string;
    directory?: string;
    signal?: AbortSignal;
}

export async function createChildSession(args: ChildSessionSpawnArgs): Promise<unknown> {
    const creating = Promise.resolve(
        args.client.session.create({
            body: {
                ...(args.parentSessionId ? { parentID: args.parentSessionId } : {}),
                title: args.title,
            },
            query: { directory: args.directory },
            ...(args.signal ? { signal: args.signal } : {}),
        } as never),
    );
    try {
        return await withTimeout(
            creating,
            childSessionLifecycleTimeoutMs,
            "child session create timed out",
        );
    } catch (error) {
        // The caller never learns a child id after a timeout, so a create that settles later would leave an orphan; its cleanup is attached here.
        if (error instanceof TimeoutError) {
            void creating
                .then((response) => {
                    const created = normalizeSDKResponse(response, null as { id?: string } | null, {
                        preferResponseOnMissingData: true,
                    });
                    if (typeof created?.id === "string" && created.id.length > 0) {
                        return deleteChildSession(args.client, created.id);
                    }
                    return undefined;
                })
                .catch(() => undefined);
        }
        throw error;
    }
}

/** Deletion is best-effort cleanup: the caller has already finished with the child, so a failed or slow delete is logged by the caller and never blocks it past the deadline. */
export async function deleteChildSession(
    client: ChildSessionDeleteClient,
    sessionId: string,
    signal?: AbortSignal,
): Promise<void> {
    await withTimeout(
        Promise.resolve(
            client.session.delete({
                path: { id: sessionId },
                ...(signal ? { signal } : {}),
            } as never),
        ),
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
    signal?: AbortSignal,
): () => Promise<unknown[]> {
    return async () => {
        const messagesResponse = await withTimeout(
            Promise.resolve(
                client.session.messages({
                    path: { id: sessionId },
                    query: { directory, limit },
                    ...(signal ? { signal } : {}),
                } as never),
            ),
            HOST_SDK_READ_TIMEOUT_MS,
            "child session transcript read timed out",
        );
        return normalizeSDKResponse(messagesResponse, [] as unknown[], {
            preferResponseOnMissingData: true,
        });
    };
}
