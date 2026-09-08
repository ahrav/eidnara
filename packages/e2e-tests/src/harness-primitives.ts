/**
 * Harness fixture primitives: the default mock response, the SDK session surface the harness
 * client exposes, and the option fields the harness accepts.
 */

import type { MockResponse } from "./mock-provider/server";

/**
 * Default response used when the mock queue is empty. Lets tests send extra
 * prompts without worrying about scripting every one.
 */
export const DEFAULT_MOCK_RESPONSE: MockResponse = {
    text: "ok",
    usage: {
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_input_tokens: 100,
        cache_read_input_tokens: 0,
    },
};

export interface SharedHarnessOptions {
    /** Eidnara config overrides. Merged onto test defaults. */
    eidnaraConfig?: Record<string, unknown>;
    /** Extra opencode.json config. Merged onto test defaults. */
    openCodeConfigExtra?: Record<string, unknown>;
    /** Override the mock model's context token limit. Default 200000. */
    modelContextLimit?: number;
    /** Default response used when the mock queue is empty. */
    mockDefault?: MockResponse;
}

/** SDK session surface the harness client exposes; `SdkClient` intersects `session` with extra endpoints. */
export interface SdkClientCore {
    session: {
        create: (opts: {
            query: { directory: string };
            body?: { parentID?: string; title?: string };
        }) => Promise<{ data?: { id: string } }>;
        prompt: (opts: {
            path: { id: string };
            body: {
                model: { providerID: string; modelID: string };
                parts: Array<{ type: "text"; text: string }>;
                agent?: string;
            };
        }) => Promise<{ data?: unknown; error?: unknown; response?: { status?: number } }>;
        messages: (opts: { path: { id: string } }) => Promise<{ data?: unknown }>;
    };
}
