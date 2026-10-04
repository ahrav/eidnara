import type { EidnaraConfig } from "@eidnara/opencode/config/schema/eidnara";
import { DEFAULT_PROTECTED_TAGS } from "@eidnara/opencode/features/context/defaults";
import {
    resolveCacheTtl,
    resolveExecuteThreshold,
} from "@eidnara/opencode/hooks/context/event-resolvers";
import {
    passUsage,
    resolveHistoryBudgetTokens,
    transformGeometryForWire,
    transformPassFields,
} from "@eidnara/opencode/hooks/context/transform-pass-fields";
import {
    type PromptSurfaceRuntime,
    promptSurfaceWireFields,
} from "@eidnara/opencode/shared/prompt-surface-runtime";
import type { WindowGeometryResult } from "@eidnara/opencode/shared/window-geometry";

type Json = Record<string, unknown>;

export interface PiPassFieldsArgs {
    config: EidnaraConfig;
    promptSurfaceRuntime?: PromptSurfaceRuntime;
    geometry: WindowGeometryResult | undefined;
    providerId: string | null;
    modelKey: string | null;
    systemPromptHash: string;
    reduceRegistered: boolean;
    todowriteRegistered: boolean;
    /** The captured window rows' messages, oldest first. */
    messages: readonly Json[];
    now: number;
}

function count(value: unknown): number {
    return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function lastAssistantUsage(
    messages: readonly Json[],
): { usage: Json; timestamp: unknown } | undefined {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        const message = messages[index];
        if (message?.role !== "assistant" || message.stopReason === "error") continue;
        const usage = message.usage;
        if (usage !== null && typeof usage === "object")
            return { usage: usage as Json, timestamp: message.timestamp };
    }
    return undefined;
}

export function piPassFields(args: PiPassFieldsArgs): Record<string, unknown> {
    const { config, geometry, modelKey } = args;
    const contextLimit = geometry?.usableSoft;
    const latest = lastAssistantUsage(args.messages);
    const cacheRead = count(latest?.usage.cacheRead);
    const cacheWrite = count(latest?.usage.cacheWrite);
    const inputTokens = count(latest?.usage.input) + cacheRead + cacheWrite;
    const usage =
        latest && contextLimit
            ? {
                  percentage: (inputTokens / contextLimit) * 100,
                  inputTokens,
                  cache: { readTokens: cacheRead, writeTokens: cacheWrite },
              }
            : undefined;
    const threshold = resolveExecuteThreshold(
        config.execute_threshold_percentage ?? 65,
        modelKey ?? undefined,
        65,
        { tokensConfig: config.execute_threshold_tokens, contextLimit: contextLimit ?? 128_000 },
    );
    return transformPassFields({
        passInputs: {
            effective_execute_threshold: threshold,
            auto_search_enabled: config.memory?.auto_search?.enabled ?? true,
            auto_search_score_threshold: config.memory?.auto_search?.score_threshold ?? 0.6,
            auto_search_min_prompt_chars: config.memory?.auto_search?.min_prompt_chars ?? 20,
            history_budget_tokens: resolveHistoryBudgetTokens(
                config.history_budget_percentage,
                usage ?? { percentage: 0, inputTokens: 0 },
                config.execute_threshold_percentage,
                modelKey ?? undefined,
                config.execute_threshold_tokens,
                contextLimit,
            ),
            clear_reasoning_age: config.clear_reasoning_age ?? 50,
            terse_text_compression_enabled: config.terse_text_compression?.enabled === true,
            terse_text_compression_min_chars: config.terse_text_compression?.min_chars ?? 500,
            cache_ttl: resolveCacheTtl(config.cache_ttl, modelKey ?? undefined),
            is_subagent: false,
            tool_present: args.reduceRegistered,
            todo_tool_present: args.todowriteRegistered,
            ...promptSurfaceWireFields(args.promptSurfaceRuntime, config.prompt_surface, modelKey),
            protected_tags: config.protected_tags ?? DEFAULT_PROTECTED_TAGS,
        },
        usage: usage && contextLimit ? passUsage(usage, contextLimit) : undefined,
        prevResponseCacheUsage: usage && {
            cache_read_tokens: cacheRead,
            cache_write_tokens: cacheWrite,
        },
        geometry: transformGeometryForWire(geometry),
        modelKey,
        providerId: args.providerId,
        systemPromptHash: args.systemPromptHash,
        midTurn: args.messages.at(-1)?.role === "toolResult",
        prevResponseCompletedAtMs:
            typeof latest?.timestamp === "number" && latest.timestamp > 0
                ? latest.timestamp
                : undefined,
        requestObservedAtMs: args.now,
    });
}
