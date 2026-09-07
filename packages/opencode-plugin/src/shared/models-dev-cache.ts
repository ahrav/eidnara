/**
 * OpenCode sessions resolve per-model limits only through OpenCode's SDK.
 *
 * Use `client.config.providers()` to resolve OpenCode session limits.
 * Avoid direct reads of `models.json`; a concurrent write can expose partial data.
 *
 * Layers:
 * Warm `apiCache` once at startup from the SDK.
 * Seed `apiCache` from persisted values so restarts retain limits before SDK warming completes.
 *
 * Clamp cached values to [20,000, 3,000,000] before returning or persisting them.
 * Retry startup warming when OpenCode's provider service is unavailable.
 *
 * Pi resolves its limit through `ctx.getModel().contextWindow`, not `getSdkContextLimit()`.
 * `getSdkContextLimit()` returns `undefined` for Pi.
 */

import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ContextLimitProvenance } from "./context-limit-provenance";
import { getEidnaraStorageDir } from "./data-path";
import { getHarness } from "./harness";
import { modelRefLookupOrder } from "./harness-provider-map";
import { sessionLog } from "./logger";
import {
    deriveWindowGeometry,
    getWindowOverlay,
    isSaneLimit,
    resolveWindowOverlayFacts,
    type WindowGeometryResult,
} from "./window-geometry";

interface OpencodeClientLike {
    config: {
        providers: () => Promise<{ data?: { providers?: unknown } }>;
    };
}

// Pi and OpenCode reject the same out-of-range values through this one bound.
export { isSaneLimit, MAX_SANE_LIMIT, MIN_SANE_LIMIT } from "./window-geometry";

export type OutputReserveConfig = number | { default: number; [modelKey: string]: number };

export interface ModelLimit {
    context?: number;
    input?: number;
    output?: number;
}

interface CachedModelMetadata {
    /** Legacy resolved value, retained so pre-upgrade persisted caches remain readable. */
    limit?: number;
    /** Raw combined context window. Reservation is applied only when the value is read. */
    contextLimit?: number;
    /** Provider-enforced prompt cap. Undefined when only a combined context window is known. */
    inputLimit?: number;
    /** Maximum generated tokens advertised by the provider/model catalog. */
    outputLimit?: number;
    /** Provider metadata says the model accepts image input. Unknown is false. */
    vision?: boolean;
}

let outputReserveConfig: OutputReserveConfig | undefined;

/**
 * `apiCache` is populated asynchronously from OpenCode's SDK.
 * `client.config.providers()` is the resolved OpenCode configuration source.
 * Ignore persisted values after `apiCache` has SDK data.
 * Pi does not populate apiCache because it resolves limits from contextWindow; it falls through to the file fallback.
 */
let apiCache: Map<string, CachedModelMetadata> | null = null;
let apiLoadedAt = 0;

// The persisted OpenCode apiCache survives restarts, so cold starts use limits before SDK warm-up.
// Only OpenCode warms and persists apiCache; Pi does not seed it.
let persistSeedLoaded = false;

function persistFilePath(): string {
    return join(getEidnaraStorageDir(), `model-context-limits-${getHarness()}.json`);
}

/** Seeding before SDK warm-up preserves last-known-good limits across restarts.
 * Invalid persisted metadata is discarded before it enters apiCache.
 * */
function loadPersistedApiCacheOnce(): void {
    if (persistSeedLoaded || apiCache !== null) return;
    persistSeedLoaded = true;
    try {
        const raw = readFileSync(persistFilePath(), "utf-8");
        const obj = JSON.parse(raw) as Record<
            string,
            | number
            | {
                  limit?: number;
                  contextLimit?: number;
                  inputLimit?: number;
                  outputLimit?: number;
                  vision?: boolean;
              }
        >;
        const map = new Map<string, CachedModelMetadata>();
        for (const [key, persisted] of Object.entries(obj)) {
            const limit = typeof persisted === "number" ? persisted : persisted.limit;
            const contextLimit = typeof persisted === "number" ? undefined : persisted.contextLimit;
            const inputLimit = typeof persisted === "number" ? undefined : persisted.inputLimit;
            const outputLimit = typeof persisted === "number" ? undefined : persisted.outputLimit;
            const vision = typeof persisted === "number" ? false : persisted.vision === true;
            if (isSaneLimit(contextLimit) || isSaneLimit(limit)) {
                map.set(key, {
                    limit: isSaneLimit(limit) ? limit : undefined,
                    contextLimit: isSaneLimit(contextLimit) ? contextLimit : undefined,
                    inputLimit: isSaneLimit(inputLimit) ? inputLimit : undefined,
                    outputLimit: isFinitePositive(outputLimit) ? outputLimit : undefined,
                    vision,
                });
            }
        }
        if (map.size > 0) {
            apiCache = map;
            sessionLog(
                "global",
                `models-dev-cache: seeded ${map.size} entries from persisted cache (cold start)`,
            );
        }
    } catch {
        // Persisted-cache read failures leave apiCache unset.
    }
}

/** Temp-write and rename prevent readers from observing a torn file.
 * */
function persistApiCache(): void {
    if (!apiCache) return;
    const obj: Record<string, CachedModelMetadata> = {};
    for (const [key, value] of apiCache) {
        if (isSaneLimit(value.limit)) {
            obj[key] = {
                limit: value.limit,
                contextLimit: isSaneLimit(value.contextLimit) ? value.contextLimit : undefined,
                inputLimit: isSaneLimit(value.inputLimit) ? value.inputLimit : undefined,
                outputLimit: isFinitePositive(value.outputLimit) ? value.outputLimit : undefined,
                vision: value.vision === true,
            };
        }
    }
    const target = persistFilePath();
    const tmp = `${target}.${process.pid}.tmp`;
    try {
        mkdirSync(getEidnaraStorageDir(), { recursive: true });
        // `writeFileSync` applies `mode` only when it creates the file, so a stale
        // temp file from an interrupted write must go first or its mode survives.
        rmSync(tmp, { force: true });
        writeFileSync(tmp, JSON.stringify(obj), { encoding: "utf-8", mode: 0o600 });
        renameSync(tmp, target);
        chmodSync(target, 0o600);
    } catch {
        // A failed persist loses only cold-start cache warmth, not correctness.
        try {
            rmSync(tmp, { force: true });
        } catch {
            // Best effort; the next write removes the temp file before reuse.
        }
    }
}

function isFinitePositive(value: number | undefined): value is number {
    return typeof value === "number" && Number.isFinite(value) && value > 0;
}

const VISION_MARKER = /image|vision/i;

/** A matching key with `false` does not indicate vision support. */
function hasVisionMarker(value: unknown): boolean {
    if (typeof value === "string") return VISION_MARKER.test(value);
    if (Array.isArray(value)) return value.some(hasVisionMarker);
    if (typeof value !== "object" || value === null) return false;
    return Object.entries(value).some(([key, entry]) =>
        typeof entry === "boolean" ? entry && VISION_MARKER.test(key) : hasVisionMarker(entry),
    );
}

function modelKeyLookupOrder(providerID: string, modelID: string): string[] {
    const candidates = [...modelRefLookupOrder(`${providerID}/${modelID}`), modelID];
    const colon = modelID.lastIndexOf(":");
    if (colon > 0) {
        const bareModel = modelID.slice(0, colon);
        candidates.push(...modelRefLookupOrder(`${providerID}/${bareModel}`), bareModel);
    }
    return [...new Set(candidates)];
}

/* */
export function resolveOutputReserve(
    providerID: string,
    modelID: string,
    config: OutputReserveConfig | undefined = outputReserveConfig,
): number | undefined {
    if (typeof config === "number")
        return Number.isFinite(config) && config >= 0 ? config : undefined;
    if (!config) return undefined;
    for (const candidate of modelKeyLookupOrder(providerID, modelID)) {
        const value = config[candidate];
        if (typeof value === "number" && Number.isFinite(value) && value >= 0) return value;
    }
    return Number.isFinite(config.default) && config.default >= 0 ? config.default : undefined;
}

/** Set the user-tier reservation override shared by every resolved-limit consumer. */
export function setOutputReserveConfig(config: OutputReserveConfig | undefined): void {
    outputReserveConfig = config;
}

/** Delegates so the provider geometry table, output cap ratio, and half-window floor exist once. */
export function resolveLimit(
    limit: ModelLimit | undefined,
    providerID: string,
    modelID: string,
    reserveConfig: OutputReserveConfig | undefined = outputReserveConfig,
): number | undefined {
    return deriveWindowGeometry(providerID, modelID, limit, {
        outputReserveOverride: resolveOutputReserve(providerID, modelID, reserveConfig),
    })?.usableSoft;
}

function setCachedModelMetadata(
    cache: Map<string, CachedModelMetadata>,
    key: string,
    model:
        | {
              limit?: ModelLimit;
              experimental?: { modes?: Record<string, unknown> };
              capabilities?: unknown;
              modalities?: unknown;
              input?: unknown;
              attachment?: unknown;
          }
        | undefined,
): void {
    const contextLimit = model?.limit?.context;
    const inputLimit = model?.limit?.input;
    const outputLimit = model?.limit?.output;
    const rawLimit = isSaneLimit(contextLimit)
        ? contextLimit
        : isSaneLimit(inputLimit)
          ? inputLimit
          : undefined;

    // Raw-metadata validation precedes reservation so a valid raw limit remains cacheable after output reservation.
    if (rawLimit === undefined) return;

    const vision = [model?.capabilities, model?.modalities, model?.input, model?.attachment].some(
        hasVisionMarker,
    );
    const value: CachedModelMetadata = {
        // The sane raw limit remains the fallback when no reserved limit is usable.
        // The resolver resolves context, input, and output limits at use time so user overrides remain live.
        limit: rawLimit,
        contextLimit: isSaneLimit(contextLimit) ? contextLimit : undefined,
        inputLimit: isSaneLimit(inputLimit) ? inputLimit : undefined,
        outputLimit: isFinitePositive(outputLimit) ? outputLimit : undefined,
        vision,
    };
    cache.set(key, value);

    // OpenCode creates derived model IDs from experimental.modes
    // Derived IDs such as gpt-5.4-fast inherit their parent model's context limit.
    // An explicit catalog row for a derived ID takes precedence regardless of catalog order.
    const modes = model?.experimental?.modes;
    if (modes && typeof modes === "object") {
        for (const mode of Object.keys(modes)) {
            const derivedKey = `${key}-${mode}`;
            if (!cache.has(derivedKey)) cache.set(derivedKey, value);
        }
    }
}

/**
 *
 * Plugin startup and authentication recovery refresh model metadata.
 * The provider endpoint supplies resolved model metadata.
 *
 * The loader retries empty provider responses so startup can populate the limit cache.
 * At startup, `config.providers()` can return no providers.
 *
 */
export async function refreshModelLimitsFromApi(
    client: OpencodeClientLike,
    options?: { retries?: number; retryDelayMs?: number },
): Promise<void> {
    const attempts = Math.max(1, (options?.retries ?? 0) + 1);
    const delayMs = options?.retryDelayMs ?? 1000;
    for (let attempt = 1; attempt <= attempts; attempt++) {
        const ok = await refreshModelLimitsOnce(client);
        if (ok) return;
        if (attempt < attempts) {
            await new Promise((resolve) => setTimeout(resolve, delayMs));
        }
    }
}

let authRewarmDone = false;

/**
 * After a successful warm, `authRewarmDone` prevents further `config.providers()` calls until reset.
 *
 * Authentication-specific limits can differ from unauthenticated catalog limits.
 * The startup cache can contain unauthenticated limits.
 *
 * `authRewarmDone` is set before the await to suppress concurrent refreshes.
 * A failed refresh clears `authRewarmDone` so a later call can retry.
 * A startup refresh still in flight when this runs cannot overwrite the
 * authenticated result: `refreshModelLimitsOnce` applies results in request order.
 */
export async function refreshModelLimitsAfterAuthOnce(client: OpencodeClientLike): Promise<void> {
    if (authRewarmDone) return;
    authRewarmDone = true;
    const ok = await refreshModelLimitsOnce(client);
    if (!ok) authRewarmDone = false;
}

/* */
export function resetAuthRewarmLatchForTest(): void {
    authRewarmDone = false;
}

/** A result whose request started before the last applied request is discarded. */
let refreshGeneration = 0;
let appliedGeneration = 0;

/* */
async function refreshModelLimitsOnce(client: OpencodeClientLike): Promise<boolean> {
    const generation = ++refreshGeneration;
    try {
        const result = await client.config.providers();
        const data = (result as { data?: { providers?: Array<unknown> } }).data;
        const providers = data?.providers;
        if (!Array.isArray(providers) || providers.length === 0) {
            sessionLog(
                "global",
                "models-dev-cache: API refresh returned no providers payload (will retry if attempts remain)",
            );
            return false;
        }

        const map = new Map<string, CachedModelMetadata>();
        for (const entry of providers) {
            const p = entry as {
                id?: string;
                models?: Record<
                    string,
                    {
                        limit?: ModelLimit;
                        experimental?: { modes?: Record<string, unknown> };
                    }
                >;
            };
            if (!p?.id || !p.models || typeof p.models !== "object") continue;
            for (const [modelId, model] of Object.entries(p.models)) {
                setCachedModelMetadata(map, `${p.id}/${modelId}`, model);
            }
        }
        if (map.size === 0) {
            sessionLog(
                "global",
                "models-dev-cache: API refresh returned providers without a usable model limit; keeping the last-known-good cache (will retry if attempts remain)",
            );
            return false;
        }
        if (generation < appliedGeneration) {
            sessionLog(
                "global",
                `models-dev-cache: discarded a stale API refresh of ${map.size} entries because a later refresh already applied`,
            );
            return true;
        }
        appliedGeneration = generation;

        const previousSize = apiCache?.size ?? null;
        apiCache = map;
        apiLoadedAt = Date.now();
        // `persistApiCache` preserves sane-filtered limits for the next cold start.
        persistApiCache();

        if (previousSize === null) {
            sessionLog(
                "global",
                `models-dev-cache: API layer loaded ${map.size} model metadata entries`,
            );
        } else if (previousSize !== map.size) {
            sessionLog(
                "global",
                `models-dev-cache: API layer loaded ${map.size} model metadata entries (was ${previousSize})`,
            );
        }
        return true;
    } catch (error) {
        sessionLog(
            "global",
            "models-dev-cache: API refresh failed:",
            error instanceof Error ? error.message : String(error),
        );
        return false;
    }
}

/**
 * The resolver uses OpenCode's `config.providers()` SDK result for prompt limits.
 * The resolver does not read OpenCode's `models.json` file directly.
 * A read of that file during a write can produce invalid limits.
 *
 * Resolution:
 * Cold start seeds `apiCache` from the persisted last-known-good file once.
 * The resolver converts raw SDK metadata into an output-reserved usable limit.
 * `undefined` leaves fallback and retry behavior to the caller.
 *
 * Pi resolves limits from `ctx.model.contextWindow` instead of warming `apiCache`.
 * Pi uses `ctx.model.contextWindow` when this function returns `undefined`.
 */
/**
 * `limit` represents raw context only for legacy rows without `contextLimit`
 * or `inputLimit`. When `inputLimit` is present, `limit` is not a combined
 * window; reading it as one would reserve output against a pre-carved cap.
 */
function rawContextOf(metadata: CachedModelMetadata): number | undefined {
    if (metadata.contextLimit !== undefined) return metadata.contextLimit;
    return metadata.inputLimit === undefined ? metadata.limit : undefined;
}

/**
 * A `prompt_only` detection is a provider prompt cap, so it joins the input
 * candidates and the smallest wins. Any other detection is a downward cap on
 * the combined window. `promptCap` is set only for the prompt-only case;
 * callers use it to cap `usableHard`.
 */
function splitDetectedLimit(
    metadata: CachedModelMetadata,
    detectedContextLimit: number | undefined,
    provenance: ContextLimitProvenance | undefined,
): { input: number | undefined; contextCap: number | undefined; promptCap: number | undefined } {
    const detected = isFinitePositive(detectedContextLimit) ? detectedContextLimit : undefined;
    if (detected === undefined) {
        return { input: metadata.inputLimit, contextCap: undefined, promptCap: undefined };
    }
    if (provenance === "prompt_only") {
        const candidates = [metadata.inputLimit, detected].filter(isFinitePositive);
        const promptCap = Math.min(...candidates);
        return { input: promptCap, contextCap: undefined, promptCap };
    }
    return { input: metadata.inputLimit, contextCap: detected, promptCap: undefined };
}

export function getSdkWindowGeometry(
    providerID: string,
    modelID: string,
    detectedContextLimit?: number,
    options?: {
        detectedLimitProvenance?: ContextLimitProvenance;
        harness?: "opencode" | "pi";
    },
): WindowGeometryResult | undefined {
    loadPersistedApiCacheOnce();
    const metadata = lookupMetadataWithTagFallback(apiCache, providerID, modelID);
    if (!metadata) return undefined;
    const rawContext = rawContextOf(metadata);
    const { input, contextCap, promptCap } = splitDetectedLimit(
        metadata,
        detectedContextLimit,
        options?.detectedLimitProvenance,
    );
    const result = deriveWindowGeometry(
        providerID,
        modelID,
        {
            context: rawContext,
            input,
            output: metadata.outputLimit,
        },
        {
            overlay: resolveWindowOverlayFacts(providerID, modelID, getWindowOverlay()),
            outputReserveOverride: resolveOutputReserve(providerID, modelID),
            harness: options?.harness ?? "opencode",
            contextCap,
        },
    );
    if (!result || promptCap === undefined) return result;
    return {
        ...result,
        usableHard: Math.max(result.usableSoft, Math.min(result.usableHard, promptCap)),
    };
}

export function getSdkContextLimit(
    providerID: string,
    modelID: string,
    detectedContextLimit?: number,
    options?: {
        reservation?: "default" | "none";
        detectedLimitProvenance?: ContextLimitProvenance;
    },
): number | undefined {
    if (options?.reservation !== "none") {
        return getSdkWindowGeometry(providerID, modelID, detectedContextLimit, {
            detectedLimitProvenance: options?.detectedLimitProvenance,
        })?.usableSoft;
    }
    loadPersistedApiCacheOnce();
    const metadata = lookupMetadataWithTagFallback(apiCache, providerID, modelID);
    if (!metadata) return undefined;
    const rawContext = rawContextOf(metadata);
    const { input, contextCap } = splitDetectedLimit(
        metadata,
        detectedContextLimit,
        options?.detectedLimitProvenance,
    );
    const context =
        contextCap === undefined
            ? rawContext
            : isFinitePositive(rawContext)
              ? Math.min(rawContext, contextCap)
              : contextCap;
    return resolveLimit(
        {
            context,
            input,
            output: metadata.outputLimit,
        },
        providerID,
        modelID,
        0,
    );
}

/**
 */
/** Image-input support uses the same models.dev metadata cache as limits. */
export function modelSupportsVision(providerID: string, modelID: string): boolean {
    loadPersistedApiCacheOnce();
    if (!apiCache) return false;
    const exact = apiCache.get(`${providerID}/${modelID}`);
    if (exact?.vision === true) return true;
    const colon = modelID.lastIndexOf(":");
    return colon > 0
        ? apiCache.get(`${providerID}/${modelID.slice(0, colon)}`)?.vision === true
        : false;
}

export function getSdkInputLimit(providerID: string, modelID: string): number | undefined {
    loadPersistedApiCacheOnce();
    const direct = lookupMetadataWithTagFallback(apiCache, providerID, modelID)?.inputLimit;
    return isSaneLimit(direct) ? direct : undefined;
}

/**
 * Exact lookup precedes fallback so models with tagged metadata remain distinct.
 *
 */
function lookupMetadataWithTagFallback(
    cache: Map<string, CachedModelMetadata> | null,
    providerID: string,
    modelID: string,
): CachedModelMetadata | undefined {
    if (!cache) return undefined;
    const exact = cache.get(`${providerID}/${modelID}`);
    if (exact) return exact;

    const colonIdx = modelID.lastIndexOf(":");
    if (colonIdx > 0) {
        return cache.get(`${providerID}/${modelID.slice(0, colonIdx)}`);
    }
    return undefined;
}

/* */
export function clearModelsDevCache(): void {
    apiCache = null;
    apiLoadedAt = 0;
    persistSeedLoaded = false;
}

/* */
export function getModelsDevCacheState(): {
    apiLoaded: boolean;
    apiCount: number;
    apiAgeMs: number;
} {
    return {
        apiLoaded: apiCache !== null,
        apiCount: apiCache?.size ?? 0,
        apiAgeMs: apiLoadedAt > 0 ? Date.now() - apiLoadedAt : -1,
    };
}
