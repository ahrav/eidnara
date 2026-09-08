import { normalizeSDKResponse } from "../../shared/normalize-sdk-response";

interface ChildSessionClient {
    session: { create(input: never): unknown | Promise<unknown> };
}

interface ChildSessionSpawnArgs {
    client: ChildSessionClient;
    parentSessionId?: string;
    title: string;
    directory?: string;
    signal?: AbortSignal;
}

export async function createChildSession(args: ChildSessionSpawnArgs): Promise<unknown> {
    return args.client.session.create({
        body: {
            ...(args.parentSessionId ? { parentID: args.parentSessionId } : {}),
            title: args.title,
        },
        query: { directory: args.directory },
        ...(args.signal ? { signal: args.signal } : {}),
    } as never);
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
