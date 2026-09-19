import { BoundedSessionMap } from "../../shared/bounded-session-map";
import type { PromptSurfaceConfig } from "../../shared/prompt-surface";
import {
    type PromptSurfaceRuntime,
    promptSurfaceWireFields,
} from "../../shared/prompt-surface-runtime";
import { isRecord } from "../../shared/record-type-guard";
import type { RustModeModuleClient } from "./rust-mode-transform";
import type { SessionDirectoryResolver } from "./session-directory";
import type { GuidanceFetchArgs } from "./system-prompt-hash";

/** `GUIDANCE_FETCH_TIMEOUT_MS` matches `TRANSFORM_SEND_TIMEOUT_MS` in `module-transport.ts`. */
const GUIDANCE_FETCH_TIMEOUT_MS = 5_000;

const GUIDANCE_SESSION_CAPACITY = 1000;

export interface GuidanceFetcherDeps {
    moduleClient: RustModeModuleClient;
    projectRootForSession: SessionDirectoryResolver;
    promptSurfaceRuntime: PromptSurfaceRuntime | undefined;
    promptSurface: PromptSurfaceConfig | undefined;
    language: string | undefined;
}

export interface GuidanceFetcher {
    (args: GuidanceFetchArgs): Promise<string | undefined>;
    clearSession(sessionId: string): void;
}

/**
 * If a cache-busting refetch fails, the fetcher returns cached bytes for the same variant so
 * the system prompt keeps the block it already carried.
 */
export function createGuidanceFetcher(deps: GuidanceFetcherDeps): GuidanceFetcher {
    const bySession = new BoundedSessionMap<{ key: string; bytes: string }>(
        GUIDANCE_SESSION_CAPACITY,
    );
    const fetchGuidance = async (args: GuidanceFetchArgs): Promise<string | undefined> => {
        const key = `${args.toolPresent}|${args.modelKey ?? ""}`;
        const cached = bySession.get(args.sessionId);
        const reusable = cached && cached.key === key ? cached.bytes : undefined;
        if (reusable !== undefined && !args.isCacheBusting) return reusable;
        try {
            const projectRoot = await deps.projectRootForSession(args.sessionId);
            const response = await deps.moduleClient.call({
                sessionId: args.sessionId,
                projectRoot,
                method: "guidance.get",
                signal: AbortSignal.timeout(GUIDANCE_FETCH_TIMEOUT_MS),
                body: {
                    method: "guidance.get",
                    v: 1,
                    session_id: args.sessionId,
                    tool_present: args.toolPresent,
                    serializer_profile: "opencode-aisdk",
                    ...promptSurfaceWireFields(
                        deps.promptSurfaceRuntime,
                        deps.promptSurface,
                        args.modelKey,
                    ),
                    language: deps.language,
                },
            });
            if (!isRecord(response) || typeof response.bytes !== "string") {
                throw new Error("guidance.get returned no bytes");
            }
            bySession.set(args.sessionId, { key, bytes: response.bytes });
            return response.bytes;
        } catch (error) {
            if (reusable !== undefined) return reusable;
            throw error;
        }
    };
    return Object.assign(fetchGuidance, {
        clearSession: (sessionId: string) => {
            bySession.delete(sessionId);
        },
    });
}
