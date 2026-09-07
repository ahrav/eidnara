import type { EidnaraPluginConfig } from "../../config";
import { DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE } from "../../config/schema/eidnara";
import { createCompactionHandler } from "../../features/context/compaction";
import { DEFAULT_PROTECTED_TAGS } from "../../features/context/defaults";
import { createScheduler } from "../../features/context/scheduler";
import { createTagger } from "../../features/context/tagger";
import { createEidnaraHookAsync } from "../../hooks/context";
import type { LiveSessionState } from "../../hooks/context/live-session-state";
import type { RustModeModuleClient } from "../../hooks/context/rust-mode-transform";
import type { PromptSurfaceRuntime } from "../../shared/prompt-surface-runtime";
import type { PluginContext } from "../types";
/**
 */
export function buildEidnaraHookConfig(pluginConfig: EidnaraPluginConfig) {
    // The spread preserves future hook-config fields without mapper changes.
    return {
        ...pluginConfig,
        protected_tags: pluginConfig.protected_tags ?? DEFAULT_PROTECTED_TAGS,
        execute_threshold_percentage:
            pluginConfig.execute_threshold_percentage ?? DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE,
    };
}

export async function createSessionHooksAsync(args: {
    ctx: PluginContext;
    pluginConfig: EidnaraPluginConfig;
    liveSessionState: LiveSessionState;
    rustModeModuleClient?: RustModeModuleClient;
    promptSurfaceRuntime?: PromptSurfaceRuntime;
}) {
    const { ctx, pluginConfig, liveSessionState } = args;

    if (pluginConfig.enabled !== true) {
        return { eidnara: null, rustToolBackends: undefined };
    }

    const tagger = createTagger();
    const scheduler = createScheduler({
        executeThresholdPercentage:
            pluginConfig.execute_threshold_percentage ?? DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE,
        executeThresholdTokens: pluginConfig.execute_threshold_tokens,
    });
    const compactionHandler = createCompactionHandler();
    const hookResult = await createEidnaraHookAsync({
        client: ctx.client,
        directory: ctx.directory,
        tagger,
        scheduler,
        compactionHandler,
        liveSessionState,
        rustModeModuleClient: args.rustModeModuleClient,
        promptSurfaceRuntime: args.promptSurfaceRuntime,
        config: buildEidnaraHookConfig(pluginConfig),
    });

    return {
        eidnara: hookResult,
        rustToolBackends: hookResult?.rustToolBackends,
    };
}
