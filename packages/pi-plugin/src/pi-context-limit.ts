import {
    type OutputReserveConfig,
    resolveOutputReserve,
} from "@eidnara/opencode/shared/models-dev-cache";
import {
    deriveWindowGeometry,
    getWindowOverlay,
    resolveWindowOverlayFacts,
    type WindowGeometryResult,
} from "@eidnara/opencode/shared/window-geometry";

const MIN_SANE_LIMIT = 16_000;
const MAX_SANE_LIMIT = 10_000_000;

export interface PiModelLimit {
    provider?: string;
    id?: string;
    contextWindow?: number;
    maxTokens?: number;
}

export interface ResolvePiWindowGeometryArgs {
    rawContextWindow?: number;
    model?: PiModelLimit;
    detectedContextLimit?: number;
    persistedInputTokens?: number;
    persistedPercentage?: number;
    reserveConfig?: OutputReserveConfig;
}

function isSaneLimit(value: number | undefined): value is number {
    return (
        typeof value === "number" &&
        Number.isFinite(value) &&
        value >= MIN_SANE_LIMIT &&
        value <= MAX_SANE_LIMIT
    );
}

function isFinitePositive(value: number | undefined): value is number {
    return typeof value === "number" && Number.isFinite(value) && value > 0;
}

export function resolvePiWindowGeometry(
    args: ResolvePiWindowGeometryArgs,
): WindowGeometryResult | undefined {
    const runtimeWindow = isSaneLimit(args.rawContextWindow)
        ? args.rawContextWindow
        : isSaneLimit(args.model?.contextWindow)
          ? args.model.contextWindow
          : undefined;
    // The sanity bounds apply to the inferred window, not to the sample's token count.
    const persistedUsable =
        isFinitePositive(args.persistedInputTokens) && isFinitePositive(args.persistedPercentage)
            ? args.persistedInputTokens / (args.persistedPercentage / 100)
            : undefined;
    const contextCap = isSaneLimit(args.detectedContextLimit)
        ? args.detectedContextLimit
        : undefined;
    // A detected overflow is the only window a provider ever reports for an uncatalogued model.
    const context =
        runtimeWindow ?? (isSaneLimit(persistedUsable) ? persistedUsable : undefined) ?? contextCap;
    if (!isSaneLimit(context)) return undefined;
    const providerID = args.model?.provider ?? "unknown";
    const modelID = args.model?.id ?? "unknown";
    const outputReserveOverride = resolveOutputReserve(providerID, modelID, args.reserveConfig);
    const result = deriveWindowGeometry(
        providerID,
        modelID,
        {
            context,
            output: args.model?.maxTokens,
        },
        {
            overlay: resolveWindowOverlayFacts(providerID, modelID, getWindowOverlay()),
            contextCap,
            outputReserveOverride,
            harness: "pi",
        },
    );
    if (!result || outputReserveOverride !== undefined || !isSaneLimit(persistedUsable))
        return result;
    const usableSoft = Math.round(persistedUsable);
    // `result.usableHard` already reflects the runtime window and any detected overflow.
    // A persisted estimate above `result.usableHard` cannot postpone compaction past the wall.
    if (usableSoft > result.usableHard) return result;
    return {
        ...result,
        usableSoft,
        derivation: {
            ...result.derivation,
            reserve: Math.max(0, result.derivation.window - usableSoft),
        },
    };
}

export function resolvePiUsableContextLimit(args: ResolvePiWindowGeometryArgs): number | undefined {
    return resolvePiWindowGeometry(args)?.usableSoft;
}
