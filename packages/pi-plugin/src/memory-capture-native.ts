import { streamSimple } from "@earendil-works/pi-ai/compat";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
    NativeCaptureError,
    type NativeCaptureExecutor,
} from "@eidnara/opencode/shared/memory-capture";
import { resolveModelRefForHost } from "./subagent-runner";

/** Provider auth and transport failures say nothing about the model's output, so they must not
 * spend the daemon's model-failure allowance. Pi surfaces them only as `errorMessage` text at
 * this pin; the classes match Pi's own retry matcher plus credential rejections. */
const PROVIDER_UNAVAILABLE =
    /\b(?:401|403|429|5\d\d)\b|unauthori[sz]ed|forbidden|invalid.?api.?key|authentication|rate.?limit|too many requests|overloaded|service.?unavailable|server.?error|network|connection|fetch failed|socket|timed? ?out|timeout|terminated/i;

function failureCode(message: string | undefined, signal: AbortSignal): NativeCaptureError["code"] {
    if (signal.aborted) return "cancelled";
    return PROVIDER_UNAVAILABLE.test(message ?? "") ? "provider_unavailable" : "model_failed";
}

/** Resolve auth inside the active harness, including its OAuth refresh and
 * registered custom providers. Neither credentials nor reasoning leave it. */
export function piMemoryCaptureExecutor(ctx: ExtensionContext): NativeCaptureExecutor {
    return async (work, signal) => {
        // The daemon's configured chain names canonical refs (`openai/...`); this host registers
        // the same provider under its own name (`openai-codex`). The lease keeps the daemon's ref.
        const native = resolveModelRefForHost(work.model);
        const separator = native.indexOf("/");
        const provider = native.slice(0, separator);
        const id = native.slice(separator + 1);
        const model =
            ctx.model?.provider === provider && ctx.model.id === id
                ? ctx.model
                : ctx.modelRegistry.find(provider, id);
        if (!model) throw new NativeCaptureError("provider_unavailable");
        const controller = new AbortController();
        const abort = () => controller.abort();
        signal.addEventListener("abort", abort, { once: true });
        if (signal.aborted) abort();
        try {
            const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
            if (!auth.ok) throw new NativeCaptureError("provider_unavailable");
            if (controller.signal.aborted) throw new NativeCaptureError("cancelled");
            const stream = streamSimple(
                model,
                {
                    systemPrompt: work.system,
                    messages: [{ role: "user", content: work.prompt, timestamp: Date.now() }],
                    tools: [],
                },
                {
                    apiKey: auth.apiKey,
                    headers: auth.headers,
                    env: auth.env,
                    signal: controller.signal,
                    maxTokens: Math.max(
                        1,
                        Math.min(
                            work.maxOutputTokens,
                            model.maxTokens,
                            Math.floor(model.contextWindow / 4),
                        ),
                    ),
                    timeoutMs: work.maxDurationMs,
                    maxRetries: 0,
                },
            );
            let text = "";
            let bytes = 0;
            for await (const event of stream) {
                if (event.type === "text_delta") {
                    bytes += Buffer.byteLength(event.delta);
                    if (bytes > work.maxOutputBytes) {
                        controller.abort();
                        throw new NativeCaptureError("output_limit");
                    }
                    text += event.delta;
                }
                if (event.type === "error")
                    throw new NativeCaptureError(failureCode(event.error.errorMessage, signal));
                if (event.type === "done" && event.reason !== "stop")
                    throw new NativeCaptureError(
                        event.reason === "length" ? "output_limit" : "model_failed",
                    );
            }
            if (controller.signal.aborted) throw new NativeCaptureError("cancelled");
            const result = await stream.result();
            if (result.provider !== provider || result.model !== id || result.stopReason !== "stop")
                throw new NativeCaptureError("model_failed");
            return { model: work.model, text };
        } catch (error) {
            controller.abort();
            throw error instanceof NativeCaptureError
                ? error
                : new NativeCaptureError(
                      failureCode(error instanceof Error ? error.message : undefined, signal),
                  );
        } finally {
            signal.removeEventListener("abort", abort);
        }
    };
}
