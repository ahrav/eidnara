import type { createOpencodeClient } from "@opencode-ai/sdk";

import { detectOverflow, extractErrorMessage } from "../features/context/overflow-detection";
import { log } from "./logger";
import { parseProviderModel } from "./resolve-fallbacks";

type Client = ReturnType<typeof createOpencodeClient>;

/**
 * The 3-second limit prevents a wedged abort endpoint from masking the original timeout or abort error. */
const ABORT_CALL_TIMEOUT_MS = 3000;

const DEFAULT_TIMEOUT_MS = 300_000;
const EXTERNAL_ABORT_MESSAGE = "prompt aborted by external signal";
const TIMEOUT_MESSAGE_PATTERN = /^(?:prompt|output) timed out after \d+ms$/;

const MAX_SUGGESTION_SEARCH_DEPTH = 6;
const NESTED_ERROR_KEYS = ["data", "error", "cause", "body"] as const;

export type PromptBody = {
    model?: { providerID: string; modelID: string };
    [key: string]: unknown;
};

export type PromptArgs = {
    path: { id: string };
    body: PromptBody;
    signal?: AbortSignal;
    [key: string]: unknown;
};

/**
 * A prompt facade may mutate nested request data such as `body.parts` before rejecting.
 * `structuredClone` gives each attempt its own copy of the whole JSON-compatible body.
 */
function cloneBody(body: PromptBody): PromptBody {
    return structuredClone(body);
}

function copyPromptArgs(args: PromptArgs, body: PromptBody): PromptArgs {
    return { ...args, body: cloneBody(body) };
}

/**
 * `PromptArgs.signal` is the SDK's own cancellation field, so a caller may supply it there instead of in options.
 */
function composeSignals(
    primary: AbortSignal | undefined,
    secondary: AbortSignal | undefined,
): AbortSignal | undefined {
    if (!primary) return secondary;
    if (!secondary || primary === secondary) return primary;
    return AbortSignal.any([primary, secondary]);
}

export interface PromptAttemptInfo {
    /** `label` identifies the model in logs as `primary` or `provider/model`. */
    label: string;
    /** Zero-based attempt index: 0 is primary, 1+ are fallback models. */
    attemptIndex: number;
    /** `isFallback` is true for configured fallbacks and false for the primary attempt. */
    isFallback: boolean;
    /** `totalAttempts` includes the primary and every configured fallback. */
    totalAttempts: number;
    /** `model` overrides the model for this attempt when supplied. */
    model?: { providerID: string; modelID: string };
}

export interface PromptRetryOptions {
    /** `timeoutMs` applies separately to each attempt's prompt phase and fetch-and-validate phase. */
    timeoutMs?: number;
    /**
     * External abort signal cancels the in-flight prompt and the fetch-and-validate phase;
     * `fetchOutput` receives a linked signal in `args.signal`.
     * A signal supplied in `PromptArgs.signal` also cancels both phases; either signal cancels when both are present.
     */
    signal?: AbortSignal;
    /**
     * `fallbackModels` lists alternates to try after the primary attempt fails.
     * Empty or undefined `fallbackModels` disables fallback iteration.
     *
     * Fallback policy:
     * Each attempt retries a `did you mean X?` error once.
     * Abort, timeout, and context-overflow errors stop fallback iteration.
     */
    fallbackModels?: readonly string[];
    /**
     * `callContext` identifies the call site in structured logs.
     * Structured logs use `callContext` to correlate fallback attempts with a call site.
     * `callContext` defaults to `subagent`.
     */
    callContext?: string;
}

export interface ValidatedPromptRetryOptions<TOutput, TValidated> extends PromptRetryOptions {
    /**
     * OpenCode exposes results through session messages.
     * Each caller validates a different output shape.
     * `args.signal` aborts when the attempt's deadline passes or the external signal fires.
     */
    fetchOutput: (args: PromptArgs, attempt: PromptAttemptInfo) => Promise<TOutput>;
    /**
     * A thrown validation error rejects the model output and advances to the next fallback.
     * Validation errors are never classified as transport failures, whatever their message says.
     */
    validateOutput: (
        output: TOutput,
        attempt: PromptAttemptInfo,
    ) => TValidated | Promise<TValidated>;
}

export interface ValidatedPromptRetryResult<TOutput, TValidated> {
    output: TOutput;
    validated: TValidated;
    attempt: PromptAttemptInfo;
}

export interface ModelSuggestionInfo {
    providerID: string;
    modelID: string;
    suggestion: string;
}

/** `OutputValidationError` prevents `isNonRetryable` from classifying validation errors by their messages. */
class OutputValidationError extends Error {
    constructor(cause: unknown) {
        super(extractErrorMessage(cause), { cause });
        this.name = "OutputValidationError";
    }
}

function unwrapValidationError(error: unknown): unknown {
    return error instanceof OutputValidationError ? error.cause : error;
}

function parseModelSuggestion(
    error: unknown,
    seen: WeakSet<object> = new WeakSet(),
    depth = 0,
): ModelSuggestionInfo | null {
    if (!error) return null;

    if (typeof error === "object") {
        // The visited set and depth cap terminate traversal of cyclic error graphs.
        if (seen.has(error) || depth >= MAX_SUGGESTION_SEARCH_DEPTH) return null;
        seen.add(error);
        const errObj = error as Record<string, unknown>;

        if (
            errObj.name === "ProviderModelNotFoundError" &&
            typeof errObj.data === "object" &&
            errObj.data !== null
        ) {
            const data = errObj.data as Record<string, unknown>;
            const suggestions = data.suggestions;
            if (Array.isArray(suggestions) && typeof suggestions[0] === "string") {
                return {
                    providerID: String(data.providerID ?? ""),
                    modelID: String(data.modelID ?? ""),
                    suggestion: suggestions[0],
                };
            }
        }

        for (const key of NESTED_ERROR_KEYS) {
            const nested = errObj[key];
            if (nested && typeof nested === "object") {
                const result = parseModelSuggestion(nested, seen, depth + 1);
                if (result) return result;
            }
        }
    }

    const message = extractErrorMessage(error);
    const modelMatch = message.match(/model not found:\s*([^/\s]+)\s*\/\s*([^.,\s]+)/i);
    const suggestionMatch = message.match(/did you mean:\s*([^,?]+)/i);

    if (!modelMatch || !suggestionMatch) {
        return null;
    }

    return {
        providerID: modelMatch[1].trim(),
        modelID: modelMatch[2].trim(),
        suggestion: suggestionMatch[1].trim(),
    };
}

// Without `throwOnError`, the SDK client resolves HTTP errors as `{ error }` instead of rejecting.
function resolvedSdkError(result: unknown): unknown {
    if (result && typeof result === "object" && "error" in result) {
        return (result as { error?: unknown }).error;
    }
    return undefined;
}

type DeadlinePhase = "prompt" | "output";

/**
 * `Promise.race` returns when the attempt is cancelled, even if `run` ignores its signal.
 * `onCancel` releases remote work before the timeout or abort error is thrown; its failure must not mask that error.
 */
async function runWithDeadline<T>(
    run: (signal: AbortSignal) => Promise<T>,
    timeoutMs: number,
    signal: AbortSignal | undefined,
    phase: DeadlinePhase,
    onCancel?: () => Promise<void>,
): Promise<T> {
    if (signal?.aborted) {
        throw new Error(EXTERNAL_ABORT_MESSAGE);
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    const onExternalAbort = () => controller.abort();
    signal?.addEventListener("abort", onExternalAbort);
    const cancelled = new Promise<never>((_resolve, reject) => {
        controller.signal.addEventListener("abort", () => reject(new Error("cancelled")), {
            once: true,
        });
    });

    try {
        return await Promise.race([run(controller.signal), cancelled]);
    } catch (error) {
        if (signal?.aborted) {
            await onCancel?.();
            throw new Error(EXTERNAL_ABORT_MESSAGE);
        }
        if (controller.signal.aborted) {
            await onCancel?.();
            throw new Error(`${phase} timed out after ${timeoutMs}ms`);
        }
        throw error;
    } finally {
        clearTimeout(timer);
        signal?.removeEventListener("abort", onExternalAbort);
    }
}

function promptWithTimeout(
    client: Client,
    args: PromptArgs,
    timeoutMs: number,
    signal?: AbortSignal,
): Promise<void> {
    return runWithDeadline(
        async (attemptSignal) => {
            const result = await client.session.prompt({
                ...args,
                signal: attemptSignal,
            } as Parameters<typeof client.session.prompt>[0]);
            const error = resolvedSdkError(result);
            if (error) throw error;
        },
        timeoutMs,
        signal,
        "prompt",
        () => abortChildRun(client, args.path.id),
    );
}

/**
 * The child session continues running after a prompt timeout or external abort.
 * An abort-session failure must not mask the original timeout or abort error.
 */
async function abortChildRun(client: Client, sessionId: string): Promise<void> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
        // The 3-second cleanup timeout prevents cleanup from delaying the original timeout or abort error.
        const result = await Promise.race([
            client.session.abort({ path: { id: sessionId } }),
            new Promise<void>((resolve) => {
                timer = setTimeout(resolve, ABORT_CALL_TIMEOUT_MS);
            }),
        ]);
        // The non-throwing SDK mode resolves an HTTP failure as `{ error }`, leaving the child running.
        const error = resolvedSdkError(result);
        if (error) throw error;
    } catch (error) {
        log(`[model-retry] child session abort failed for ${sessionId}: ${String(error)}`);
    } finally {
        clearTimeout(timer);
    }
}

/**
 * Abort, timeout, and context-overflow errors stop fallback iteration.
 */
function isNonRetryable(error: unknown, externalSignal?: AbortSignal): boolean {
    if (externalSignal?.aborted) return true;
    if (error instanceof OutputValidationError) return false;

    if (error instanceof Error) {
        if (error.name === "AbortError") return true;
        if (error.message === EXTERNAL_ABORT_MESSAGE) return true;
        if (TIMEOUT_MESSAGE_PATTERN.test(error.message)) return true;
    }

    if (detectOverflow(error).isOverflow) return true;

    return false;
}

function shortErr(error: unknown): string {
    const message = extractErrorMessage(error);
    if (error instanceof Error && error.name && error.name !== "Error") {
        return `${error.name}: ${message}`;
    }
    return message;
}

/**
 * The function retries once when the SDK suggests a replacement model.
 * The returned args carry the body the successful prompt used, so `body.model` names the effective model.
 */
async function attemptOnce(
    client: Client,
    args: PromptArgs,
    timeoutMs: number,
    signal: AbortSignal | undefined,
    callContext: string,
    label: string,
): Promise<PromptArgs> {
    // `originalBody` is never handed to the facade, so it stays pristine for the suggested retry.
    // `originalBody.model` identifies the model that the suggested retry replaces.
    const originalBody = cloneBody(args.body);
    const attemptArgs = copyPromptArgs(args, originalBody);
    try {
        await promptWithTimeout(client, attemptArgs, timeoutMs, signal);
        return attemptArgs;
    } catch (error) {
        if (isNonRetryable(error, signal)) throw error;

        let suggestion: ModelSuggestionInfo | null = null;
        try {
            suggestion = parseModelSuggestion(error);
        } catch {
            // A parser fault must not replace the transport error.
            suggestion = null;
        }
        if (!suggestion || !originalBody.model) {
            // The caller's fallback loop selects the next model when no suggested model is available.
            throw error;
        }

        log(`[${callContext}] ${label}: model not found, retrying with suggestion`, {
            original: `${suggestion.providerID}/${suggestion.modelID}`,
            suggested: suggestion.suggestion,
        });

        const retryArgs = copyPromptArgs(args, {
            ...originalBody,
            model: {
                providerID: suggestion.providerID,
                modelID: suggestion.suggestion,
            },
        });
        await promptWithTimeout(client, retryArgs, timeoutMs, signal);
        return retryArgs;
    }
}

/** A suggested-model retry changes the effective model, so the attempt info must name it. */
function withEffectiveModel(info: PromptAttemptInfo, body: PromptBody): PromptAttemptInfo {
    const model = body.model;
    if (
        !model ||
        (model.providerID === info.model?.providerID && model.modelID === info.model.modelID)
    ) {
        return info;
    }
    return { ...info, model, label: `${model.providerID}/${model.modelID}` };
}

interface FallbackRun<T> {
    args: PromptArgs;
    options: PromptRetryOptions;
    attempt: (attemptArgs: PromptArgs, info: PromptAttemptInfo) => Promise<T>;
    terminalError: "first" | "last";
    failureNoun: string;
}

interface PlannedFallback {
    label: string;
    model: { providerID: string; modelID: string };
}

/**
 * Deduplicating on the parsed pair bills each provider/model once per run and keeps `totalAttempts` honest.
 */
function planFallbacks(fallbacks: readonly string[], callContext: string): PlannedFallback[] {
    const seen = new Set<string>();
    const plan: PlannedFallback[] = [];
    for (const spec of fallbacks) {
        const parsed = parseProviderModel(spec);
        if (!parsed) {
            log(`[${callContext}] skipping invalid fallback spec: ${spec}`);
            continue;
        }
        const label = `${parsed.providerID}/${parsed.modelID}`;
        if (seen.has(label)) {
            log(`[${callContext}] skipping duplicate fallback spec: ${spec}`);
            continue;
        }
        seen.add(label);
        plan.push({ label, model: parsed });
    }
    return plan;
}

async function runWithFallbacks<T>(run: FallbackRun<T>): Promise<T> {
    const { args, options } = run;
    const callContext = options.callContext ?? "subagent";
    const fallbacks = planFallbacks(options.fallbackModels ?? [], callContext);
    // `baseBody` is never handed to the facade, so fallbacks inherit no request-body mutations.
    const baseBody = cloneBody(args.body);
    const baseArgs = copyPromptArgs(args, baseBody);
    const totalAttempts = fallbacks.length + 1;

    const primaryLabel =
        baseBody.model?.providerID && baseBody.model.modelID
            ? `${baseBody.model.providerID}/${baseBody.model.modelID}`
            : "primary";

    let firstError: unknown = null;
    let lastError: unknown = null;

    try {
        return await run.attempt(baseArgs, {
            label: primaryLabel,
            attemptIndex: 0,
            isFallback: false,
            totalAttempts,
            model: baseBody.model,
        });
    } catch (error) {
        firstError = error;
        lastError = error;
        if (isNonRetryable(error, options.signal) || fallbacks.length === 0) {
            throw unwrapValidationError(error);
        }

        log(
            `[${callContext}] primary (${primaryLabel}) ${run.failureNoun}: ${shortErr(error)}; trying ${fallbacks.length} fallback(s)`,
        );
    }

    for (let i = 0; i < fallbacks.length; i += 1) {
        const { label, model } = fallbacks[i];
        const attemptArgs = copyPromptArgs(baseArgs, { ...baseBody, model });

        try {
            const result = await run.attempt(attemptArgs, {
                label,
                attemptIndex: i + 1,
                isFallback: true,
                totalAttempts,
                model,
            });
            log(
                `[${callContext}] fallback succeeded with ${label} (attempt ${i + 2}/${totalAttempts})`,
            );
            return result;
        } catch (error) {
            lastError = error;
            if (isNonRetryable(error, options.signal)) throw unwrapValidationError(error);

            const remaining = fallbacks.length - i - 1;
            if (remaining > 0) {
                log(
                    `[${callContext}] ${label} ${run.failureNoun}: ${shortErr(error)}; ${remaining} fallback(s) left`,
                );
            }
        }
    }

    log(
        `[${callContext}] all models exhausted; tried: ${[primaryLabel, ...fallbacks.map((f) => f.label)].join(", ")}; original error: ${shortErr(firstError)}; last error: ${shortErr(lastError)}`,
    );
    const terminal = run.terminalError === "first" ? firstError : lastError;
    throw unwrapValidationError(terminal) ?? new Error("All fallback models failed");
}

/**
 * The function tries the resolved primary model before `options.fallbackModels`.
 * Each attempt retries once when the SDK suggests a replacement model.
 * `isNonRetryable` errors stop fallback retries.
 *
 * With no fallback models, only the model-suggestion retry runs.
 * When every model fails, the last attempt's error is thrown.
 */
export function promptSyncWithModelSuggestionRetry(
    client: Client,
    args: PromptArgs,
    options: PromptRetryOptions = {},
): Promise<void> {
    const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    const callContext = options.callContext ?? "subagent";
    const signal = composeSignals(options.signal, args.signal);
    return runWithFallbacks<void>({
        args,
        options: { ...options, signal },
        attempt: async (attemptArgs, info) => {
            await attemptOnce(client, attemptArgs, timeoutMs, signal, callContext, info.label);
        },
        terminalError: "last",
        failureNoun: "failed",
    });
}

/**
 * When every model fails, the primary attempt's error is thrown.
 * Every attempt prompts `args.path.id`; the caller owns session creation and decides whether fallbacks share a session.
 */
export function promptSyncWithValidatedOutputRetry<TOutput, TValidated = TOutput>(
    client: Client,
    args: PromptArgs,
    options: ValidatedPromptRetryOptions<TOutput, TValidated>,
): Promise<ValidatedPromptRetryResult<TOutput, TValidated>> {
    const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    const callContext = options.callContext ?? "subagent";
    const signal = composeSignals(options.signal, args.signal);
    return runWithFallbacks<ValidatedPromptRetryResult<TOutput, TValidated>>({
        args,
        options: { ...options, signal },
        attempt: async (attemptArgs, info) => {
            const promptedArgs = await attemptOnce(
                client,
                attemptArgs,
                timeoutMs,
                signal,
                callContext,
                info.label,
            );
            // A suggested-model retry may have prompted a different model than `info` names.
            const effectiveInfo = withEffectiveModel(info, promptedArgs.body);
            return runWithDeadline(
                async (phaseSignal) => {
                    const output = await options.fetchOutput(
                        { ...promptedArgs, signal: phaseSignal },
                        effectiveInfo,
                    );
                    let validated: TValidated;
                    try {
                        validated = await options.validateOutput(output, effectiveInfo);
                    } catch (error) {
                        throw new OutputValidationError(error);
                    }
                    return { output, validated, attempt: effectiveInfo };
                },
                timeoutMs,
                signal,
                "output",
            );
        },
        terminalError: "first",
        failureNoun: "failed validation/prompt",
    });
}
