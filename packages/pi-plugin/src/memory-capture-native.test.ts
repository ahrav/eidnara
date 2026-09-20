import { afterEach, expect, it } from "bun:test";
import {
    type Api,
    type AssistantMessage,
    createAssistantMessageEventStream,
    type Model,
    registerApiProvider,
    unregisterApiProviders,
} from "@earendil-works/pi-ai/compat";
import { piMemoryCaptureExecutor } from "./memory-capture-native";

const source = "eidnara-capture-test";
afterEach(() => unregisterApiProviders(source));

it("uses native custom-provider auth without exposing credentials or reasoning", async () => {
    const model: Model<Api> = {
        id: "m",
        name: "custom",
        provider: "custom-native",
        api: source,
        baseUrl: "http://127.0.0.1",
        reasoning: false,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 32000,
        maxTokens: 16000,
    };
    const result: AssistantMessage = {
        role: "assistant",
        content: [{ type: "text", text: '{"ok":true}' }],
        api: source,
        provider: model.provider,
        model: model.id,
        stopReason: "stop",
        timestamp: 0,
        usage: {
            input: 1,
            output: 1,
            cacheRead: 0,
            cacheWrite: 0,
            totalTokens: 2,
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
    };
    let calls = 0;
    const stream: Parameters<typeof registerApiProvider>[0]["streamSimple"] = (
        _model,
        context,
        options,
    ) => {
        calls++;
        expect(context.tools).toEqual([]);
        expect(options?.apiKey).toBe("native-refreshed-access");
        expect(options?.headers).toEqual({ "x-native-account": "account" });
        expect(options?.maxTokens).toBe(8000);
        expect(options?.temperature).toBeUndefined();
        const events = createAssistantMessageEventStream();
        events.push({
            type: "thinking_delta",
            contentIndex: 0,
            delta: "hidden reasoning",
            partial: result,
        });
        events.push({ type: "text_delta", contentIndex: 0, delta: '{"ok":true}', partial: result });
        events.push({ type: "done", reason: "stop", message: result });
        return events;
    };
    registerApiProvider({ api: source, stream, streamSimple: stream }, source);
    let authCalls = 0;
    const executor = piMemoryCaptureExecutor({
        modelRegistry: {
            find: (provider: string, id: string) =>
                provider === model.provider && id === model.id ? model : undefined,
            getApiKeyAndHeaders: async () => {
                authCalls++;
                return {
                    ok: true,
                    apiKey: "native-refreshed-access",
                    headers: { "x-native-account": "account" },
                };
            },
        },
    } as never);
    const output = await executor(
        {
            model: "custom-native/m",
            system: "system",
            prompt: "source",
            maxOutputTokens: 8192,
            maxOutputBytes: 131072,
            maxDurationMs: 90000,
        },
        new AbortController().signal,
    );
    expect(output).toEqual({ model: "custom-native/m", text: '{"ok":true}' });
    expect(authCalls).toBe(1);
    expect(calls).toBe(1);
});
