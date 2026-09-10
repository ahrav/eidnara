import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { escalationBands, MAX_EXECUTE_THRESHOLD } from "../../shared/escalation-bands";
import { piModelRefToCanonical } from "../../shared/harness-provider-map";
import { log, sessionLog } from "../../shared/logger";
import { getSdkContextLimit, getSdkWindowGeometry } from "../../shared/models-dev-cache";
import { modelKeyLookupOrder, resolveModelConfigOrDefault } from "../../shared/prompt-surface";

export { escalationBands, MAX_EXECUTE_THRESHOLD };
export const DEFAULT_CONTEXT_LIMIT = 128_000;

export function resolveContextWindowGeometry(
    providerID: string | undefined,
    modelID: string | undefined,
) {
    if (!providerID || !modelID) return undefined;
    return getSdkWindowGeometry(providerID, modelID, undefined, {
        detectedLimitProvenance: "unknown",
        harness: "opencode",
    });
}

type CacheTtlConfig = string | Record<string, string>;

/** Pressure percentages divide by this limit, so unknown models still get a positive value. */
export function resolveContextLimit(
    providerID: string | undefined,
    modelID: string | undefined,
    ctx?: { reservation?: "default" | "none" },
): number {
    const fromModelsDev =
        providerID && modelID
            ? getSdkContextLimit(providerID, modelID, undefined, {
                  reservation: ctx?.reservation,
                  detectedLimitProvenance: "unknown",
              })
            : undefined;
    return fromModelsDev ?? DEFAULT_CONTEXT_LIMIT;
}

/**
 * Trusted limits come from positive models.dev context limits. The 128K
 * fallback can undersize a large-context history budget, so unknown models
 * return `undefined` instead.
 */
export function resolveTrustedContextLimit(
    providerID: string | undefined,
    modelID: string | undefined,
): number | undefined {
    const fromModelsDev =
        providerID && modelID
            ? getSdkContextLimit(providerID, modelID, undefined, {
                  detectedLimitProvenance: "unknown",
              })
            : undefined;
    if (typeof fromModelsDev === "number" && fromModelsDev > 0) return fromModelsDev;
    return undefined;
}

export function resolveCacheTtl(cacheTtl: CacheTtlConfig, modelKey: string | undefined): string {
    if (typeof cacheTtl === "string") {
        return cacheTtl;
    }

    return resolveModelConfigOrDefault(cacheTtl, modelKey, cacheTtl.default ?? "5m");
}

const CACHE_TTL_UNIT_MS: Record<string, number> = { s: 1_000, m: 60_000, h: 3_600_000 };
/** The daemon's `scheduler_ttl_ms` substitutes this for a TTL `parse_cache_ttl` rejects. */
export const DEFAULT_CACHE_TTL_MS = 5 * 60 * 1_000;

/** Matches the daemon's `parse_cache_ttl` grammar so both sides agree on when a lane expires. `never` is `Infinity`; unparseable text is `undefined`. */
export function parseCacheTtlMs(ttl: string): number | undefined {
    const normalized = ttl.trim();
    if (normalized.toLowerCase() === "never") return Number.POSITIVE_INFINITY;
    const match = /^(\d+)([smh]?)$/.exec(normalized);
    if (!match) return undefined;
    return Number(match[1]) * (match[2] ? CACHE_TTL_UNIT_MS[match[2]] : 1);
}

type ExecuteThresholdConfig = number | { default: number; [modelKey: string]: number };
type ExecuteThresholdTokensConfig =
    | { default?: number; [modelKey: string]: number | undefined }
    | undefined;

export interface ExecuteThresholdOptions {
    /** `tokensConfig` overrides the percentage-based threshold when it matches `modelKey` and `context_limit` is valid.
     * */
    tokensConfig?: ExecuteThresholdTokensConfig;
    /** `context_limit` is required with `tokensConfig` to convert tokens to a percentage.
     * `context_limit` caps the execute threshold at 90% of the context limit. */
    contextLimit?: number;
    /** `sessionID` directs clamping warnings to the session log; absent IDs use the global log. */
    sessionId?: string;
}

export type ExecuteThresholdMode = "percentage" | "tokens";

export interface ExecuteThresholdDetail {
    /** Downstream calculations use the effective execute threshold, constrained to 0–90%. */
    percentage: number;
    /** The authoritative source is tokens config when its modelKey matches and context is valid; otherwise it is percentage. */
    mode: ExecuteThresholdMode;
    /** In `tokens` mode, the value is the absolute token value clamped to 90% × `contextLimit`. */
    absoluteTokens?: number;
    /** The returned source key is the matched config key, or `"default"` when the default fallback applies. */
    matchedKey?: string;
    /**
     * The returned clamping flag is true when the configured value exceeds the safe cap and is reduced.
     * In tokens mode, clamping occurs when configured tokens exceed 90% × `contextLimit`.
     * In percentage mode, clamping occurs when the configured percentage exceeds `MAX_EXECUTE_THRESHOLD` (90).
     * `clamped` reports that the configured value was reduced.
     * `clamped` is present only when a clamp occurred; otherwise it is absent.
     */
    clamped?: boolean;
    /**
     * `configuredValue` is the raw configured token count in tokens mode or percentage in percentage mode.
     * `configuredValue` is present only when `clamped` is `true`.
     */
    configuredValue?: number;
}

// Eviction re-logs an old key's warning once; that is the price of a bound under session churn.
const CLAMP_WARN_DEDUPE_MAX_ENTRIES = 1000;
const clampWarnSeen = new BoundedSessionMap<true>(CLAMP_WARN_DEDUPE_MAX_ENTRIES);

function warnClampOnce(dedupeKey: string, sessionId: string | undefined, msg: string): void {
    if (clampWarnSeen.has(dedupeKey)) return;
    clampWarnSeen.set(dedupeKey, true);
    if (sessionId) {
        sessionLog(sessionId, `WARN: ${msg}`);
    } else {
        log(`[eidnara] WARN: ${msg}`);
    }
}

function isFinitePositive(v: unknown): v is number {
    return typeof v === "number" && Number.isFinite(v) && v > 0;
}

/**
 * `resolveExecuteThresholdDetail` returns the effective percentage and authoritative config source.
 * Callers that need only the percentage can use `resolveExecuteThreshold`.
 * Callers that display `mode` must use `resolveExecuteThresholdDetail`.
 */
export function resolveExecuteThresholdDetail(
    config: ExecuteThresholdConfig,
    modelKey: string | undefined,
    fallback: number,
    options?: ExecuteThresholdOptions,
): ExecuteThresholdDetail {
    // Tokens-based resolution takes precedence for a matching token configuration with a finite positive `contextLimit`.
    // Non-finite or non-positive token values and context limits fall through to percentage resolution.
    if (options?.tokensConfig && isFinitePositive(options.contextLimit)) {
        const contextLimit = options.contextLimit;
        const tokenMatch = resolveTokensMatchWithKey(options.tokensConfig, modelKey);
        if (tokenMatch && isFinitePositive(tokenMatch.value)) {
            const cap = contextLimit * (MAX_EXECUTE_THRESHOLD / 100);
            const effectiveTokens = Math.min(tokenMatch.value, cap);
            if (effectiveTokens < tokenMatch.value) {
                warnClampOnce(
                    `${options.sessionId ?? "__global__"}|${modelKey ?? "__default__"}|${tokenMatch.value}|${cap}`,
                    options.sessionId,
                    `execute_threshold_tokens clamped: ${tokenMatch.value} → ${effectiveTokens} (${MAX_EXECUTE_THRESHOLD}% of ${contextLimit}) for ${modelKey ?? "default"}`,
                );
            }
            const percentage = (effectiveTokens / contextLimit) * 100;
            const detail: ExecuteThresholdDetail = {
                percentage: Math.min(percentage, MAX_EXECUTE_THRESHOLD),
                mode: "tokens",
                absoluteTokens: Math.floor(effectiveTokens),
                matchedKey: tokenMatch.matchedKey,
            };
            // `configuredValue` retains `tokenMatch.value` when clamping.
            if (effectiveTokens < tokenMatch.value) {
                detail.clamped = true;
                detail.configuredValue = tokenMatch.value;
            }
            return detail;
        }
    }

    let resolved: number;
    let matchedKey: string | undefined;

    if (typeof config === "number") {
        resolved = config;
    } else if (modelKey) {
        let matched: number | undefined;
        for (const { key } of modelKeyLookupOrder(modelKey)) {
            if (typeof config[key] === "number") {
                matched = config[key];
                matchedKey = key;
                break;
            }
        }
        if (matched === undefined && typeof config.default === "number") {
            resolved = config.default;
            matchedKey = "default";
        } else {
            resolved = matched ?? fallback;
        }
    } else if (typeof config.default === "number") {
        resolved = config.default;
        matchedKey = "default";
    } else {
        resolved = fallback;
    }

    if (!Number.isFinite(resolved) || resolved < 0) {
        resolved = fallback;
    }

    const cappedPercentage = Math.min(resolved, MAX_EXECUTE_THRESHOLD);
    const percentageClamped = cappedPercentage < resolved;
    if (percentageClamped) {
        warnClampOnce(
            `pct|${options?.sessionId ?? "__global__"}|${modelKey ?? "__default__"}|${resolved}`,
            options?.sessionId,
            `execute_threshold clamped ${resolved}% → ${MAX_EXECUTE_THRESHOLD}% for ${modelKey ?? "default"} (capped against the output-reserved safe window; 10% remains for mid-turn growth before the absolute 95% wall)`,
        );
    }
    const detail: ExecuteThresholdDetail = {
        percentage: cappedPercentage,
        mode: "percentage",
        matchedKey,
    };
    // `configuredValue` retains the unclamped configured percentage.
    if (percentageClamped) {
        detail.clamped = true;
        detail.configuredValue = resolved;
    }
    return detail;
}

/**
 * Callers needing `mode` or `absoluteTokens` must use `resolveExecuteThresholdDetail`.
 */
export function resolveExecuteThreshold(
    config: ExecuteThresholdConfig,
    modelKey: string | undefined,
    fallback: number,
    options?: ExecuteThresholdOptions,
): number {
    return resolveExecuteThresholdDetail(config, modelKey, fallback, options).percentage;
}

function resolveTokensMatchWithKey(
    tokensConfig: ExecuteThresholdTokensConfig,
    modelKey: string | undefined,
): { value: number; matchedKey: string } | undefined {
    if (!tokensConfig) {
        return undefined;
    }

    if (modelKey) {
        for (const { key } of modelKeyLookupOrder(modelKey)) {
            const value = tokensConfig[key];
            if (typeof value === "number") {
                return { value, matchedKey: key };
            }
        }
    }

    if (typeof tokensConfig.default === "number") {
        return { value: tokensConfig.default, matchedKey: "default" };
    }

    return undefined;
}

export function resolveModelKey(
    providerID: string | undefined,
    modelID: string | undefined,
): string | undefined {
    if (!providerID || !modelID) {
        return undefined;
    }

    return piModelRefToCanonical(`${providerID}/${modelID}`);
}

export function resolveSessionId(
    properties: { info?: unknown; sessionID?: string } | undefined,
): string | undefined {
    if (typeof properties?.sessionID === "string") {
        return properties.sessionID;
    }

    const info = properties?.info;
    if (info === null || typeof info !== "object") {
        return undefined;
    }

    const record = info as Record<string, unknown>;
    if (typeof record.sessionID === "string") {
        return record.sessionID;
    }
    if (typeof record.id === "string") {
        return record.id;
    }

    return undefined;
}
