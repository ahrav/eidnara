import type { ToolDefinition } from "@opencode-ai/plugin";
import type { EidnaraPluginConfig } from "../config";
import { isCompactionEnabled } from "../config/agent-disable";
import { resolveProjectIdentityForSession } from "../features/context/project-identity";
import { setCtxReduceRegisteredGlobally } from "../hooks/context/ctx-reduce-availability";
import { kernelClientResolver } from "../hooks/context/kernel-transport";
import type { SessionDirectoryResolver } from "../hooks/context/session-directory";
import type { PromptSurfaceConfig } from "../shared/prompt-surface";
import type { PromptSurfaceRuntime } from "../shared/prompt-surface-runtime";
import { createPromptSurfaceRuntime } from "../shared/prompt-surface-runtime";
import { CTX_MEMORY_ACTIONS, createCtxMemoryTools } from "../tools/ctx-memory";
import { createCtxNoteTools } from "../tools/ctx-note";
import { createCtxReduceTools } from "../tools/ctx-reduce";
import { createCtxSearchTools } from "../tools/ctx-search";
import { normalizeToolArgSchemas } from "./normalize-tool-arg-schemas";
import type { RustToolBackends } from "./rust-tool-backends";

/** Tool ids the registry omits when `isCompactionEnabled` reports compaction off. */
const COMPACTION_OFF_REMOVED_TOOL_IDS = ["ctx_reduce"] as const;

export function getCompactionOffRemovedToolIds(): readonly string[] {
    return COMPACTION_OFF_REMOVED_TOOL_IDS;
}

export function createToolRegistry(args: {
    pluginConfig: EidnaraPluginConfig;
    rustToolBackends: RustToolBackends;
    resolveSessionDirectory?: SessionDirectoryResolver;
    promptSurfaceRuntime?: PromptSurfaceRuntime;
    registrationPromptSurface?: PromptSurfaceConfig;
}): Record<string, ToolDefinition> {
    const { pluginConfig, rustToolBackends } = args;

    if (pluginConfig.enabled !== true) {
        return {};
    }

    const compactionOff = !isCompactionEnabled(pluginConfig);
    setCtxReduceRegisteredGlobally(!compactionOff);

    const resolveProjectPath = (directory: string) =>
        resolveProjectIdentityForSession(directory, pluginConfig.allow_home_project);

    // Registration does not depend on daemon state; each call resolves its own client.
    const kernelClient = kernelClientResolver(pluginConfig);
    const allTools: Record<string, ToolDefinition> = {
        ...(compactionOff ? {} : createCtxReduceTools({ rustToolBackends })),
        ...createCtxNoteTools({ resolveProjectPath, rustToolBackends }),
        ...createCtxSearchTools({
            kernelClient,
            resolveProjectPath,
            resolveSessionDirectory: args.resolveSessionDirectory,
        }),
        ...createCtxMemoryTools({
            kernelClient,
            resolveProjectPath,
            resolveSessionDirectory: args.resolveSessionDirectory,
            allowedActions: [...CTX_MEMORY_ACTIONS],
        }),
    };

    const promptSurfaceRuntime =
        args.promptSurfaceRuntime ??
        createPromptSurfaceRuntime({
            warn: (message) => console.warn(`[eidnara] config warning: ${message}`),
        });
    const registration = promptSurfaceRuntime.resolveRegistration(
        args.registrationPromptSurface ?? pluginConfig.prompt_surface,
    );
    const surfacedTools = Object.fromEntries(
        Object.entries(allTools).map(([toolId, definition]) => [
            toolId,
            {
                ...definition,
                description: registration.descriptionFor(toolId, definition.description ?? ""),
            },
        ]),
    ) as Record<string, ToolDefinition>;

    for (const toolDefinition of Object.values(surfacedTools)) {
        normalizeToolArgSchemas(toolDefinition);
    }

    return surfacedTools;
}
