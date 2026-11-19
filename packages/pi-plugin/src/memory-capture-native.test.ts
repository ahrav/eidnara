import { afterEach, expect, it } from "bun:test";
import {
    type Api,
    type AssistantMessage,
    createAssistantMessageEventStream,
    type Model,
    registerApiProvider,
    streamSimple,
    unregisterApiProviders,
} from "@earendil-works/pi-ai/compat";
import { piMemoryCaptureExecutor, withoutConversationCacheMarkers } from "./memory-capture-native";

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

it("resolves a canonical lease model to the host provider and returns the lease model", async () => {
    const model: Model<Api> = {
        id: "m",
        name: "codex",
        provider: "openai-codex",
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
        content: [{ type: "text", text: "ok" }],
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
    const streamed: string[] = [];
    const stream: Parameters<typeof registerApiProvider>[0]["streamSimple"] = (streamModel) => {
        streamed.push(`${streamModel.provider}/${streamModel.id}`);
        const events = createAssistantMessageEventStream();
        events.push({ type: "text_delta", contentIndex: 0, delta: "ok", partial: result });
        events.push({ type: "done", reason: "stop", message: result });
        return events;
    };
    registerApiProvider({ api: source, stream, streamSimple: stream }, source);
    const executor = piMemoryCaptureExecutor({
        modelRegistry: {
            // Pi registers the OAuth provider as `openai-codex`; the daemon's configured chain
            // names the canonical `openai/...` ref.
            find: (provider: string, id: string) =>
                provider === "openai-codex" && id === model.id ? model : undefined,
            getApiKeyAndHeaders: async () => ({ ok: true, apiKey: "k", headers: {} }),
        },
    } as never);
    const output = await executor(
        {
            model: "openai/m",
            system: "system",
            prompt: "source",
            maxOutputTokens: 8192,
            maxOutputBytes: 131072,
            maxDurationMs: 90000,
        },
        new AbortController().signal,
    );
    expect(output).toEqual({ model: "openai/m", text: "ok" });
    expect(streamed).toEqual(["openai-codex/m"]);
});

it("reports provider auth and transport failures as provider_unavailable, not model_failed", async () => {
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
    const failed = (errorMessage: string): AssistantMessage => ({
        role: "assistant",
        content: [],
        api: source,
        provider: model.provider,
        model: model.id,
        stopReason: "error",
        errorMessage,
        timestamp: 0,
        usage: {
            input: 0,
            output: 0,
            cacheRead: 0,
            cacheWrite: 0,
            totalTokens: 0,
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
    });
    let errorMessage = "";
    const stream: Parameters<typeof registerApiProvider>[0]["streamSimple"] = () => {
        const events = createAssistantMessageEventStream();
        events.push({ type: "error", reason: "error", error: failed(errorMessage) });
        return events;
    };
    registerApiProvider({ api: source, stream, streamSimple: stream }, source);
    const executor = piMemoryCaptureExecutor({
        modelRegistry: {
            find: () => model,
            getApiKeyAndHeaders: async () => ({ ok: true, apiKey: "k", headers: {} }),
        },
    } as never);
    const codeFor = async (message: string): Promise<string> => {
        errorMessage = message;
        try {
            await executor(
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
        } catch (error) {
            return (error as { code: string }).code;
        }
        return "resolved";
    };
    expect(await codeFor("401 Unauthorized: invalid API key")).toBe("provider_unavailable");
    expect(await codeFor("429 Too Many Requests")).toBe("provider_unavailable");
    expect(await codeFor("fetch failed: ECONNREFUSED")).toBe("provider_unavailable");
    expect(await codeFor("invalid_request_error: unsupported parameter")).toBe("model_failed");
});

const captureModel: Model<Api> = {
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

function captureResult(text: string): AssistantMessage {
    return {
        role: "assistant",
        content: [{ type: "text", text }],
        api: source,
        provider: captureModel.provider,
        model: captureModel.id,
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
}

it("sends a capture payload whose only cache marker follows the system prompt", async () => {
    const result = captureResult("{}");
    const sent: unknown[] = [];
    const converse = {
        system: [{ text: "instructions" }, { cachePoint: { type: "default" } }],
        messages: [
            { role: "user", content: [{ text: "sources" }, { cachePoint: { type: "default" } }] },
        ],
    };
    const stream: Parameters<typeof registerApiProvider>[0]["streamSimple"] = (
        requestModel,
        _context,
        options,
    ) => {
        const events = createAssistantMessageEventStream();
        void Promise.resolve(options?.onPayload?.(converse, requestModel)).then((payload) => {
            sent.push(payload ?? converse);
            events.push({ type: "text_delta", contentIndex: 0, delta: "{}", partial: result });
            events.push({ type: "done", reason: "stop", message: result });
        });
        return events;
    };
    registerApiProvider({ api: source, stream, streamSimple: stream }, source);
    const executor = piMemoryCaptureExecutor({
        modelRegistry: {
            find: () => captureModel,
            getApiKeyAndHeaders: async () => ({ ok: true, apiKey: "key" }),
        },
    } as never);
    await executor(
        {
            model: "custom-native/m",
            system: "instructions",
            prompt: "sources",
            maxOutputTokens: 100,
            maxOutputBytes: 1000,
            maxDurationMs: 1000,
        } as never,
        new AbortController().signal,
    );
    expect(sent).toEqual([
        {
            system: [{ text: "instructions" }, { cachePoint: { type: "default" } }],
            messages: [{ role: "user", content: [{ text: "sources" }] }],
        },
    ]);
});

it("removes cache markers from user messages and keeps the system prompt's marker", () => {
    const marker = { type: "ephemeral" };
    const anthropic = {
        system: [{ type: "text", text: "instructions", cache_control: marker }],
        messages: [
            { role: "user", content: [{ type: "text", text: "sources", cache_control: marker }] },
        ],
    };
    expect(withoutConversationCacheMarkers(anthropic)).toEqual({
        system: anthropic.system,
        messages: [{ role: "user", content: [{ type: "text", text: "sources" }] }],
    });
    // An OpenAI-completions payload with Anthropic cache control carries the system prompt as a message.
    const completions = {
        max_completion_tokens: 100,
        messages: [
            {
                role: "system",
                content: [{ type: "text", text: "instructions", cache_control: marker }],
            },
            { role: "user", content: [{ type: "text", text: "sources", cache_control: marker }] },
        ],
    };
    expect(withoutConversationCacheMarkers(completions)).toEqual({
        max_completion_tokens: 100,
        messages: [
            completions.messages[0],
            { role: "user", content: [{ type: "text", text: "sources" }] },
        ],
    });
    const developer = {
        messages: [{ role: "developer", content: [{ type: "text", text: "i", cache_control: marker }] }],
    };
    expect(withoutConversationCacheMarkers(developer)).toEqual(developer);
    const plain = { messages: [{ role: "user", content: "text" }], input: "x" };
    expect(withoutConversationCacheMarkers(plain)).toEqual(plain);
});

it("leaves the Bedrock payload Pi builds with one cache point, after the system prompt", async () => {
    const bedrock: Model<Api> = {
        ...captureModel,
        id: "anthropic.claude-sonnet-4-5",
        name: "Claude Sonnet 4.5",
        provider: "amazon-bedrock",
        api: "bedrock-converse-stream",
        baseUrl: "http://127.0.0.1:9",
    };
    let built: unknown;
    let sent: unknown;
    const stream = streamSimple(
        bedrock,
        {
            systemPrompt: "instructions",
            messages: [{ role: "user", content: "sources", timestamp: 0 }],
            tools: [],
        },
        {
            env: {
                AWS_ACCESS_KEY_ID: "test",
                AWS_SECRET_ACCESS_KEY: "test",
                AWS_REGION: "us-west-2",
            },
            maxRetries: 0,
            onPayload: (payload) => {
                built = structuredClone(payload);
                sent = withoutConversationCacheMarkers(payload);
                throw new Error("payload inspected");
            },
        },
    );
    for await (const _event of stream) void _event;
    const point = { cachePoint: { type: "default" } };
    expect(built).toMatchObject({
        system: [{ text: "instructions" }, point],
        messages: [{ role: "user", content: [{ text: "sources" }, point] }],
    });
    expect(sent).toMatchObject({
        system: [{ text: "instructions" }, point],
        messages: [{ role: "user", content: [{ text: "sources" }] }],
    });
});
