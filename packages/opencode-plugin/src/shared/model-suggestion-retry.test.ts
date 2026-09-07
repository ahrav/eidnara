import { describe, expect, mock, test } from "bun:test";

import {
    promptSyncWithModelSuggestionRetry,
    promptSyncWithValidatedOutputRetry,
} from "./model-suggestion-retry";

type PromptCall = {
    body: {
        model?: { providerID: string; modelID: string };
        system?: string;
        [key: string]: unknown;
    };
    signal?: AbortSignal;
};

function createClient(
    prompt: ReturnType<typeof mock>,
    abort?: ReturnType<typeof mock>,
    messages?: ReturnType<typeof mock>,
) {
    return {
        session: {
            prompt,
            abort: abort ?? mock(async () => ({})),
            messages: messages ?? mock(async () => []),
        },
    } as never;
}

function createArgs(model?: { providerID: string; modelID: string }) {
    return {
        path: { id: "ses-test" },
        body: model ? { model } : {},
    };
}

describe("promptSyncWithModelSuggestionRetry", () => {
    test("primary succeeds, no fallback iteration", async () => {
        const prompt = mock(async () => ({}));
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(client, createArgs(), {
            fallbackModels: ["anthropic/claude-sonnet-4-6"],
        });

        expect(prompt).toHaveBeenCalledTimes(1);
    });

    test("primary succeeds with no fallbacks configured", async () => {
        const prompt = mock(async () => ({}));
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(client, createArgs());

        expect(prompt).toHaveBeenCalledTimes(1);
    });

    test("primary fails, fallback[0] succeeds", async () => {
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) throw new Error("primary failed");
            return {};
        });
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(client, createArgs(), {
            fallbackModels: ["anthropic/claude-sonnet-4-6"],
        });

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.model).toEqual({
            providerID: "anthropic",
            modelID: "claude-sonnet-4-6",
        });
    });

    test("primary fails, fallback[0] fails, fallback[1] succeeds", async () => {
        const prompt = mock(async () => {
            if (prompt.mock.calls.length <= 2)
                throw new Error(`failed ${prompt.mock.calls.length}`);
            return {};
        });
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(client, createArgs(), {
            fallbackModels: ["anthropic/claude-sonnet-4-6", "google/gemini-3-flash"],
        });

        expect(prompt).toHaveBeenCalledTimes(3);
        expect((prompt.mock.calls[2]?.[0] as PromptCall).body.model).toEqual({
            providerID: "google",
            modelID: "gemini-3-flash",
        });
    });

    test("all attempts fail throws the last fallback error", async () => {
        const primaryError = new Error("primary failed");
        const firstFallbackError = new Error("fallback 0 failed");
        const lastFallbackError = new Error("fallback 1 failed");
        const errors = [primaryError, firstFallbackError, lastFallbackError];
        const prompt = mock(async () => {
            throw errors[prompt.mock.calls.length - 1];
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                fallbackModels: ["anthropic/claude-sonnet-4-6", "google/gemini-3-flash"],
            }),
        ).rejects.toBe(lastFallbackError);
        expect(prompt).toHaveBeenCalledTimes(3);
    });

    test("abort signal short-circuits", async () => {
        const controller = new AbortController();
        controller.abort();
        const prompt = mock(async () => {
            throw new Error("provider noticed abort");
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                signal: controller.signal,
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
            }),
        ).rejects.toThrow("prompt aborted by external signal");
        // Pre-aborted signal MUST short-circuit before any upstream prompt
        expect(prompt).toHaveBeenCalledTimes(0);
    });

    test("AbortError name short-circuits", async () => {
        const abortError = new Error("aborted by provider");
        abortError.name = "AbortError";
        const prompt = mock(async () => {
            throw abortError;
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
            }),
        ).rejects.toBe(abortError);
        expect(prompt).toHaveBeenCalledTimes(1);
    });

    test("timeout short-circuits", async () => {
        const timeoutError = new Error("prompt timed out after 5000ms");
        const prompt = mock(async () => {
            throw timeoutError;
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
            }),
        ).rejects.toBe(timeoutError);
        expect(prompt).toHaveBeenCalledTimes(1);
    });

    test("context overflow short-circuits", async () => {
        const overflowError = new Error("prompt is too long: 50000 tokens > 32000");
        const prompt = mock(async () => {
            throw overflowError;
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
            }),
        ).rejects.toBe(overflowError);
        expect(prompt).toHaveBeenCalledTimes(1);
    });

    test("timeout fires session.abort on the child session", async () => {
        const prompt = mock((opts: { signal?: AbortSignal }) => {
            return new Promise((_resolve, reject) => {
                opts.signal?.addEventListener("abort", () => reject(new Error("aborted")));
            });
        });
        const abort = mock(async () => ({}));
        const client = createClient(prompt as never, abort);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { timeoutMs: 20 }),
        ).rejects.toThrow(/timed out/);
        expect(abort).toHaveBeenCalledTimes(1);
        expect((abort.mock.calls[0]?.[0] as { path: { id: string } }).path.id).toBe("ses-test");
    });

    test("external abort fires session.abort on the child session", async () => {
        const controller = new AbortController();
        const prompt = mock((opts: { signal?: AbortSignal }) => {
            return new Promise((_resolve, reject) => {
                opts.signal?.addEventListener("abort", () => reject(new Error("aborted")));
            });
        });
        const abort = mock(async () => ({}));
        const client = createClient(prompt as never, abort);

        setTimeout(() => controller.abort(), 10);
        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { signal: controller.signal }),
        ).rejects.toThrow(/aborted by external signal/);
        expect(abort).toHaveBeenCalledTimes(1);
        expect((abort.mock.calls[0]?.[0] as { path: { id: string } }).path.id).toBe("ses-test");
    });

    // A failing session.abort must not mask the original timeout/abort error.
    test("session.abort failure does not mask the timeout error", async () => {
        const prompt = mock((opts: { signal?: AbortSignal }) => {
            return new Promise((_resolve, reject) => {
                opts.signal?.addEventListener("abort", () => reject(new Error("aborted")));
            });
        });
        const abort = mock(async () => {
            throw new Error("abort endpoint 500");
        });
        const client = createClient(prompt as never, abort);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { timeoutMs: 20 }),
        ).rejects.toThrow(/timed out/);
        expect(abort).toHaveBeenCalledTimes(1);
    });

    test("a resolved { error } from session.abort does not mask the timeout error", async () => {
        const prompt = mock((opts: { signal?: AbortSignal }) => {
            return new Promise((_resolve, reject) => {
                opts.signal?.addEventListener("abort", () => reject(new Error("aborted")));
            });
        });
        // The SDK's non-throwing mode resolves an HTTP failure instead of rejecting.
        const abort = mock(async () => ({ error: { message: "abort endpoint 500" } }));
        const client = createClient(prompt as never, abort);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { timeoutMs: 20 }),
        ).rejects.toThrow(/timed out/);
        expect(abort).toHaveBeenCalledTimes(1);
    });

    test("duplicate and whitespace-variant fallback specs are attempted once", async () => {
        const prompt = mock(async () => {
            throw new Error(`failed ${prompt.mock.calls.length}`);
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                fallbackModels: [
                    "anthropic/claude-sonnet-4-6",
                    " anthropic/claude-sonnet-4-6 ",
                    "anthropic / claude-sonnet-4-6",
                    "google/gemini-3-flash",
                    "anthropic/claude-sonnet-4-6",
                ],
            }),
        ).rejects.toThrow("failed 3");

        // primary + two distinct fallbacks
        expect(prompt).toHaveBeenCalledTimes(3);
        expect(prompt.mock.calls.map((call) => (call[0] as PromptCall).body.model)).toEqual([
            undefined,
            { providerID: "anthropic", modelID: "claude-sonnet-4-6" },
            { providerID: "google", modelID: "gemini-3-flash" },
        ]);
    });

    test("totalAttempts counts only distinct valid fallbacks", async () => {
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) throw new Error("primary failed");
            return {};
        });
        const client = createClient(prompt);
        let total = 0;

        await promptSyncWithValidatedOutputRetry(client, createArgs(), {
            fallbackModels: ["bad-spec", "a/b", "a/b", "c/d"],
            fetchOutput: async (_args, attempt) => {
                total = attempt.totalAttempts;
                return "ok";
            },
            validateOutput: (output: string) => output,
        });

        expect(total).toBe(3);
    });

    test("suggestion retry within attempt succeeds", async () => {
        const suggestionError = new Error("model not found");
        suggestionError.name = "ProviderModelNotFoundError";
        Object.assign(suggestionError, {
            data: {
                providerID: "anthropic",
                modelID: "claude-sonnet-4-6",
                suggestions: ["claude-sonnet-4-7"],
            },
        });
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) throw suggestionError;
            return {};
        });
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(
            client,
            createArgs({ providerID: "anthropic", modelID: "claude-sonnet-4-6" }),
            { fallbackModels: ["google/gemini-3-flash"] },
        );

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.model).toEqual({
            providerID: "anthropic",
            modelID: "claude-sonnet-4-7",
        });
    });

    test("invalid fallback specs are skipped", async () => {
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) throw new Error("primary failed");
            return {};
        });
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(client, createArgs(), {
            fallbackModels: ["no-slash", "/leading", "valid/model"],
        });

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.model).toEqual({
            providerID: "valid",
            modelID: "model",
        });
    });

    test("iteration order respected", async () => {
        const prompt = mock(async () => {
            throw new Error(`failed ${prompt.mock.calls.length}`);
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), {
                fallbackModels: [
                    "anthropic/claude-sonnet-4-6",
                    "google/gemini-3-flash",
                    "openrouter/qwen3-coder",
                ],
            }),
        ).rejects.toThrow("failed 4");

        expect(prompt).toHaveBeenCalledTimes(4);
        expect(prompt.mock.calls.map((call) => (call[0] as PromptCall).body.model)).toEqual([
            undefined,
            { providerID: "anthropic", modelID: "claude-sonnet-4-6" },
            { providerID: "google", modelID: "gemini-3-flash" },
            { providerID: "openrouter", modelID: "qwen3-coder" },
        ]);
    });

    test("empty fallbackModels = legacy", async () => {
        const originalError = new Error("primary failed without suggestion");
        const prompt = mock(async () => {
            throw originalError;
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { fallbackModels: [] }),
        ).rejects.toBe(originalError);
        expect(prompt).toHaveBeenCalledTimes(1);
    });

    // Without `throwOnError`, the SDK client resolves `{ error }` on HTTP errors instead of rejecting.
    test("a resolved { error } result is treated as a failed attempt", async () => {
        const sdkError = {
            name: "ProviderModelNotFoundError",
            data: { providerID: "anthropic", modelID: "nope", suggestions: [] },
        };
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) return { error: sdkError, response: {} };
            return {};
        });
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(
            client,
            createArgs({ providerID: "anthropic", modelID: "nope" }),
            { fallbackModels: ["google/gemini-3-flash"] },
        );

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.model).toEqual({
            providerID: "google",
            modelID: "gemini-3-flash",
        });
    });

    test("a resolved { error } result with no fallbacks rejects with that error", async () => {
        const sdkError = { name: "NotFoundError", data: { message: "session not found" } };
        const prompt = mock(async () => ({ error: sdkError, response: {} }));
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { fallbackModels: [] }),
        ).rejects.toBe(sdkError);
        expect(prompt).toHaveBeenCalledTimes(1);
    });

    // SDK `throwOnError` clients wrap the decoded body as `Error(message, { cause: { body, status } })`.
    test("suggestion retry reads the SDK-wrapped cause.body payload", async () => {
        const wrapped = new Error("ProviderModelNotFoundError", {
            cause: {
                body: {
                    name: "ProviderModelNotFoundError",
                    data: {
                        providerID: "anthropic",
                        modelID: "claude-sonnet-4-6",
                        suggestions: ["claude-sonnet-4-7"],
                    },
                },
                status: 400,
            },
        });
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) throw wrapped;
            return {};
        });
        const client = createClient(prompt);

        await promptSyncWithModelSuggestionRetry(
            client,
            createArgs({ providerID: "anthropic", modelID: "claude-sonnet-4-6" }),
            { fallbackModels: ["google/gemini-3-flash"] },
        );

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.model).toEqual({
            providerID: "anthropic",
            modelID: "claude-sonnet-4-7",
        });
    });

    test("a cyclic error graph surfaces the original error, not a stack overflow", async () => {
        const cyclic = new Error("upstream 500 from provider") as Error & { cause?: unknown };
        cyclic.cause = cyclic;
        const prompt = mock(async () => {
            throw cyclic;
        });
        const client = createClient(prompt);

        await expect(
            promptSyncWithModelSuggestionRetry(client, createArgs(), { fallbackModels: [] }),
        ).rejects.toBe(cyclic);
        expect(prompt).toHaveBeenCalledTimes(1);
    });

    test("timeout path leaves no live timers behind", async () => {
        const originalSetTimeout = globalThis.setTimeout;
        const originalClearTimeout = globalThis.clearTimeout;
        const live = new Set<unknown>();
        globalThis.setTimeout = ((...args: Parameters<typeof setTimeout>) => {
            const handle = originalSetTimeout(...args);
            live.add(handle);
            return handle;
        }) as typeof setTimeout;
        globalThis.clearTimeout = ((handle: Parameters<typeof clearTimeout>[0]) => {
            live.delete(handle);
            return originalClearTimeout(handle);
        }) as typeof clearTimeout;

        try {
            const prompt = mock((opts: { signal?: AbortSignal }) => {
                return new Promise((_resolve, reject) => {
                    opts.signal?.addEventListener("abort", () => reject(new Error("aborted")));
                });
            });
            const client = createClient(prompt as never);

            await expect(
                promptSyncWithModelSuggestionRetry(client, createArgs(), { timeoutMs: 20 }),
            ).rejects.toThrow(/timed out/);
        } finally {
            globalThis.setTimeout = originalSetTimeout;
            globalThis.clearTimeout = originalClearTimeout;
        }

        expect(live.size).toBe(0);
    });
});

describe("promptSyncWithValidatedOutputRetry", () => {
    test("valid first model returns without trying fallbacks", async () => {
        const prompt = mock(async () => ({}));
        const messages = mock(async () => "primary-output");
        const client = createClient(prompt, undefined, messages);

        const result = await promptSyncWithValidatedOutputRetry(client, createArgs(), {
            fallbackModels: ["anthropic/claude-sonnet-4-6"],
            fetchOutput: async () => messages(),
            validateOutput: (output: string) => {
                if (output.trim().length === 0) throw new Error("empty output");
                return output.trim();
            },
        });

        expect(result.validated).toBe("primary-output");
        expect(prompt).toHaveBeenCalledTimes(1);
        expect(messages).toHaveBeenCalledTimes(1);
    });

    test("preserves body.system when a failed Pi-shaped attempt mutates its body", async () => {
        const prompt = mock(async (args: PromptCall) => {
            if (prompt.mock.calls.length === 1) {
                // The facade consumes the request body before rejecting the primary model.
                // The fallback must receive an unconsumed request body so it includes its task prompt.
                delete args.body.system;
                throw new Error("primary failed");
            }
            return {};
        });
        const client = createClient(prompt);
        const systemPrompt = "CLASSIFY_SYSTEM_PROMPT";

        await promptSyncWithValidatedOutputRetry(
            client,
            {
                path: { id: "ses-classify" },
                body: {
                    agent: "dreamer-classifier",
                    system: systemPrompt,
                    parts: [{ type: "text", text: "classify" }],
                },
            },
            {
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
                fetchOutput: async () => "fallback-output",
                validateOutput: (output: string) => output,
            },
        );

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[0]?.[0] as PromptCall).body.system).toBeUndefined();
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.system).toBe(systemPrompt);
    });

    test("empty first model tries the next fallback", async () => {
        const prompt = mock(async () => ({}));
        const messages = mock(async () =>
            messages.mock.calls.length === 1 ? "" : "fallback-output",
        );
        const client = createClient(prompt, undefined, messages);

        const result = await promptSyncWithValidatedOutputRetry(client, createArgs(), {
            fallbackModels: ["anthropic/claude-sonnet-4-6"],
            fetchOutput: async () => messages(),
            validateOutput: (output: string, attempt) => {
                if (output.trim().length === 0)
                    throw new Error(`empty output from ${attempt.label}`);
                return output.trim();
            },
        });

        expect(result.validated).toBe("fallback-output");
        expect(prompt).toHaveBeenCalledTimes(2);
        expect(messages).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.model).toEqual({
            providerID: "anthropic",
            modelID: "claude-sonnet-4-6",
        });
    });

    test("all empty outputs surface the original validation failure", async () => {
        const prompt = mock(async () => ({}));
        const messages = mock(async () => "");
        const client = createClient(prompt, undefined, messages);

        await expect(
            promptSyncWithValidatedOutputRetry(client, createArgs(), {
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
                fetchOutput: async () => messages(),
                validateOutput: (output: string, attempt) => {
                    if (output.trim().length === 0) {
                        throw new Error(`empty output from ${attempt.label}`);
                    }
                    return output.trim();
                },
            }),
        ).rejects.toThrow("empty output from primary");

        expect(prompt).toHaveBeenCalledTimes(2);
        expect(messages).toHaveBeenCalledTimes(2);
    });

    test("all validation failures rethrow the caller's original error object", async () => {
        const prompt = mock(async () => ({}));
        const validationError = new Error("empty output");
        const client = createClient(prompt);

        await expect(
            promptSyncWithValidatedOutputRetry(client, createArgs(), {
                fallbackModels: ["anthropic/claude-sonnet-4-6"],
                fetchOutput: async () => "",
                validateOutput: () => {
                    throw validationError;
                },
            }),
        ).rejects.toBe(validationError);
    });

    test("a validation error quoting overflow phrasing still advances to the next fallback", async () => {
        const prompt = mock(async () => ({}));
        const client = createClient(prompt);
        const modelOutput = "Sorry, prompt is too long: 210000 tokens > 200000 maximum";

        const result = await promptSyncWithValidatedOutputRetry(client, createArgs(), {
            fallbackModels: ["anthropic/claude-sonnet-4-6"],
            fetchOutput: async (_args, attempt) => (attempt.isFallback ? "ok" : modelOutput),
            validateOutput: (output: string) => {
                if (output.startsWith("Sorry")) throw new Error(`invalid output: ${output}`);
                return output;
            },
        });

        expect(result.validated).toBe("ok");
        expect(prompt).toHaveBeenCalledTimes(2);
    });

    test("a hung fetchOutput is bounded by timeoutMs", async () => {
        const prompt = mock(async () => ({}));
        const client = createClient(prompt);

        const outcome = promptSyncWithValidatedOutputRetry(client, createArgs(), {
            timeoutMs: 20,
            fetchOutput: () => new Promise<string>(() => {}),
            validateOutput: (output: string) => output,
        }).then(
            () => "resolved",
            (error: unknown) => error,
        );
        const settled = await Promise.race([
            outcome,
            new Promise<string>((resolve) => setTimeout(() => resolve("still pending"), 500)),
        ]);

        expect(settled).toBeInstanceOf(Error);
        expect(String(settled)).toMatch(/timed out/);
    });

    test("external abort during fetchOutput rejects and reaches fetchOutput's signal", async () => {
        const controller = new AbortController();
        const prompt = mock(async () => ({}));
        const client = createClient(prompt);
        let observedAbort = false;

        const outcome = promptSyncWithValidatedOutputRetry(client, createArgs(), {
            signal: controller.signal,
            fetchOutput: (args) =>
                new Promise<string>((_resolve, reject) => {
                    args.signal?.addEventListener("abort", () => {
                        observedAbort = true;
                        reject(new Error("fetch aborted"));
                    });
                }),
            validateOutput: (output: string) => output,
        }).then(
            () => "resolved",
            (error: unknown) => error,
        );
        setTimeout(() => controller.abort(), 10);
        const settled = await Promise.race([
            outcome,
            new Promise<string>((resolve) => setTimeout(() => resolve("still pending"), 500)),
        ]);

        expect(settled).toBeInstanceOf(Error);
        expect(String(settled)).toMatch(/aborted by external signal/);
        expect(observedAbort).toBe(true);
    });

    test("a suggested-model retry reports the effective model to fetchOutput, validateOutput, and the result", async () => {
        const suggestionError = new Error("model not found");
        suggestionError.name = "ProviderModelNotFoundError";
        Object.assign(suggestionError, {
            data: {
                providerID: "anthropic",
                modelID: "claude-sonnet-4-6",
                suggestions: ["claude-sonnet-4-7"],
            },
        });
        const prompt = mock(async () => {
            if (prompt.mock.calls.length === 1) throw suggestionError;
            return {};
        });
        const client = createClient(prompt);
        const seen: Array<{ argsModel?: unknown; infoModel?: unknown; label: string }> = [];

        const result = await promptSyncWithValidatedOutputRetry(
            client,
            createArgs({ providerID: "anthropic", modelID: "claude-sonnet-4-6" }),
            {
                fetchOutput: async (args, attempt) => {
                    seen.push({
                        argsModel: args.body.model,
                        infoModel: attempt.model,
                        label: attempt.label,
                    });
                    return "ok";
                },
                validateOutput: (output: string, attempt) => {
                    seen.push({ infoModel: attempt.model, label: attempt.label });
                    return output;
                },
            },
        );

        const suggested = { providerID: "anthropic", modelID: "claude-sonnet-4-7" };
        expect(prompt).toHaveBeenCalledTimes(2);
        expect(seen[0]).toEqual({
            argsModel: suggested,
            infoModel: suggested,
            label: "anthropic/claude-sonnet-4-7",
        });
        expect(seen[1]).toEqual({ infoModel: suggested, label: "anthropic/claude-sonnet-4-7" });
        expect(result.attempt.model).toEqual(suggested);
        expect(result.attempt.label).toBe("anthropic/claude-sonnet-4-7");
        expect(result.attempt.attemptIndex).toBe(0);
        expect(result.attempt.isFallback).toBe(false);
    });

    test("a failed attempt that mutates nested body.parts does not leak into the fallback", async () => {
        const prompt = mock(async (args: PromptCall) => {
            if (prompt.mock.calls.length === 1) {
                const parts = args.body.parts as Array<{ type: string; text: string }>;
                parts.length = 0;
                throw new Error("primary failed");
            }
            return {};
        });
        const client = createClient(prompt);
        const originalParts = [{ type: "text", text: "classify" }];
        const args = {
            path: { id: "ses-classify" },
            body: { parts: originalParts },
        };

        await promptSyncWithValidatedOutputRetry(client, args, {
            fallbackModels: ["anthropic/claude-sonnet-4-6"],
            fetchOutput: async () => "fallback-output",
            validateOutput: (output: string) => output,
        });

        expect(prompt).toHaveBeenCalledTimes(2);
        expect((prompt.mock.calls[0]?.[0] as PromptCall).body.parts).toEqual([]);
        expect((prompt.mock.calls[1]?.[0] as PromptCall).body.parts).toEqual([
            { type: "text", text: "classify" },
        ]);
        // The caller's own body is never handed to the facade.
        expect(originalParts).toEqual([{ type: "text", text: "classify" }]);
    });

    test("a signal supplied only in PromptArgs.signal is honored", async () => {
        const controller = new AbortController();
        controller.abort();
        const prompt = mock(async () => ({}));
        const client = createClient(prompt);

        await expect(
            promptSyncWithValidatedOutputRetry(
                client,
                { ...createArgs(), signal: controller.signal },
                {
                    fallbackModels: ["anthropic/claude-sonnet-4-6"],
                    fetchOutput: async () => "ok",
                    validateOutput: (output: string) => output,
                },
            ),
        ).rejects.toThrow(/aborted by external signal/);

        expect(prompt).toHaveBeenCalledTimes(0);
    });

    test("either of options.signal and PromptArgs.signal cancels an in-flight prompt", async () => {
        const argsController = new AbortController();
        const optionsController = new AbortController();
        const prompt = mock(() => new Promise<never>(() => {}));
        const client = createClient(prompt);

        const outcome = promptSyncWithModelSuggestionRetry(
            client,
            { ...createArgs(), signal: argsController.signal },
            { signal: optionsController.signal, fallbackModels: ["anthropic/claude-sonnet-4-6"] },
        ).then(
            () => "resolved",
            (error: unknown) => error,
        );
        setTimeout(() => argsController.abort(), 10);
        const settled = await Promise.race([
            outcome,
            new Promise<string>((resolve) => setTimeout(() => resolve("still pending"), 500)),
        ]);

        expect(settled).toBeInstanceOf(Error);
        expect(String(settled)).toMatch(/aborted by external signal/);
        expect(prompt).toHaveBeenCalledTimes(1);
    });
});
