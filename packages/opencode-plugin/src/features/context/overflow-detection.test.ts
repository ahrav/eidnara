import { describe, expect, test } from "bun:test";
import type { ContextLimitProvenance } from "../../shared/context-limit-provenance";
import {
    detectOverflow,
    extractErrorMessage,
    MAX_SCAN_CHARS,
    parseReportedLimit,
} from "./overflow-detection";

describe("overflow-detection / extractErrorMessage", () => {
    test("returns message from Error instance", () => {
        expect(extractErrorMessage(new Error("prompt is too long"))).toBe("prompt is too long");
    });

    test("returns raw string", () => {
        expect(extractErrorMessage("context length exceeded")).toBe("context length exceeded");
    });

    test("unwraps nested provider SDK error (error.error.message)", () => {
        const nested = {
            error: { message: "Input token count 200000 exceeds the maximum of 128000" },
        };
        expect(extractErrorMessage(nested)).toContain("exceeds the maximum");
    });

    test("reads top-level message property", () => {
        expect(extractErrorMessage({ message: "prompt is too long" })).toBe("prompt is too long");
    });

    test("reads responseBody fallback", () => {
        expect(extractErrorMessage({ responseBody: "413 payload too large" })).toBe(
            "413 payload too large",
        );
    });

    test("returns empty string for null / undefined", () => {
        expect(extractErrorMessage(null)).toBe("");
        expect(extractErrorMessage(undefined)).toBe("");
    });

    test("serializes an object without recognized text fields within the scan cap", () => {
        // A decoded body far larger than the cap must not be serialized in full.
        const huge = {
            status: 400,
            data: { rows: Array.from({ length: 200_000 }, (_, i) => ({ i })) },
        };
        const started = performance.now();
        const text = extractErrorMessage(huge);
        const elapsed = performance.now() - started;

        expect(text.startsWith('{"status":400,"data":{"rows":[{"i":0},')).toBe(true);
        expect(text.length).toBeLessThanOrEqual(MAX_SCAN_CHARS + 1);
        expect(text.endsWith("…")).toBe(true);
        expect(elapsed).toBeLessThan(50);
    });

    test("serializes small objects, cycles, and bigint without throwing", () => {
        const cyclic: Record<string, unknown> = { code: 7 };
        cyclic.self = cyclic;
        expect(extractErrorMessage({ code: 7, ok: true })).toBe('{"code":7,"ok":true}');
        expect(extractErrorMessage(cyclic)).toBe('{"code":7,"self":"[cycle]"}');
        expect(extractErrorMessage({ n: 1n, f: () => 1 })).toBe('{"n":1}');
    });

    test("stops reading a wide object's properties once the cap is reached", () => {
        let reads = 0;
        const wide: Record<string, unknown> = {};
        for (let i = 0; i < 50_000; i += 1) {
            Object.defineProperty(wide, `k${i}`, {
                enumerable: true,
                get: () => {
                    reads += 1;
                    return i;
                },
            });
        }
        const text = extractErrorMessage(wide);
        expect(text.length).toBeLessThanOrEqual(MAX_SCAN_CHARS + 1);
        expect(reads).toBeLessThan(1_000);
    });
});

describe("overflow-detection / detectOverflow", () => {
    // future regex edits can't silently regress provider support.
    test.each<[string, string, number | undefined, ContextLimitProvenance | undefined]>([
        ["anthropic", "prompt is too long: 210000 tokens > 200000 maximum", 200000, "prompt_only"],
        ["bedrock", "Input is too long for requested model.", undefined, undefined],
        ["openai", "This model's maximum context length is 128000 tokens", 128000, "combined"],
        [
            "gemini",
            "Input token count 1234567 exceeds the maximum number of tokens allowed",
            undefined,
            undefined,
        ],
        [
            "xai",
            "the maximum prompt length is 256000 tokens but the prompt was 300000",
            256000,
            "prompt_only",
        ],
        ["groq", "Please reduce the length of the messages or completion", undefined, undefined],
        ["openrouter", "the maximum context length is 32768 tokens", 32768, "combined"],
        ["copilot", "Prompt exceeds the limit of 64000 tokens", 64000, "unknown"],
        ["llamacpp", "Prompt exceeds the available context size", undefined, undefined],
        ["lmstudio", "Prompt greater than the context length of the model", undefined, undefined],
        ["minimax", "context window exceeds limit", undefined, undefined],
        ["moonshot", "exceeded model token limit of 131072", undefined, undefined],
        ["generic", "context_length_exceeded", undefined, undefined],
        ["http413", "413 request entity too large", undefined, undefined],
        ["vllm", "context length is only 4096 tokens, prompt was 5000", 4096, "combined"],
        ["vllm-model", "maximum model length is 8192 tokens", 8192, "combined"],
        ["vllm2", "input length 10000 exceeds the context length of 8000", 8000, "combined"],
        ["ollama", "prompt too long; exceeded max context length", undefined, undefined],
        [
            "mistral",
            "Prompt too large for model with 32768 maximum context length",
            32768,
            "combined",
        ],
        ["zai", "model_context_window_exceeded", undefined, undefined],
        ["lemonade", "Context size has been exceeded", undefined, undefined],
    ])("%s pattern matches overflow", (_provider, message, expectedLimit, expectedProvenance) => {
        const detection = detectOverflow(message);
        expect(detection.isOverflow).toBe(true);
        expect(detection.reportedLimit).toBe(expectedLimit);
        expect(detection.reportedLimitProvenance).toBe(expectedProvenance);
    });

    test("returns not-overflow for unrelated errors", () => {
        expect(detectOverflow("Network error").isOverflow).toBe(false);
        expect(detectOverflow("Rate limit exceeded").isOverflow).toBe(false);
        expect(detectOverflow("Invalid API key").isOverflow).toBe(false);
        expect(detectOverflow("").isOverflow).toBe(false);
        expect(detectOverflow(null).isOverflow).toBe(false);
    });

    test("extracts limit through Error + nested SDK shapes end-to-end", () => {
        const nested = new Error("");
        (nested as Error & { error?: unknown }).error = {
            message: "This model's maximum context length is 128000 tokens",
        };
        const detection = detectOverflow(nested);
        expect(detection.isOverflow).toBe(true);
        expect(detection.reportedLimit).toBe(128000);
        expect(detection.reportedLimitProvenance).toBe("combined");
    });

    test("returns matchedPattern for diagnostics", () => {
        const detection = detectOverflow("prompt is too long: 210000 > 200000");
        expect(detection.isOverflow).toBe(true);
        expect(detection.matchedPattern).toBeDefined();
    });

    // HTTP and SDK wrappers can carry the provider text below a generic top-level message.
    test("finds overflow text in responseBody under a generic wrapper message", () => {
        const detection = detectOverflow({
            message: "Request failed with status code 400",
            responseBody:
                '{"error":{"message":"prompt is too long: 250000 tokens > 200000 maximum"}}',
        });
        expect(detection.isOverflow).toBe(true);
        expect(detection.reportedLimit).toBe(200000);
        expect(detection.reportedLimitProvenance).toBe("prompt_only");
    });

    test("finds overflow text in a standard Error.cause chain", () => {
        const wrapped = new Error("fetch failed", {
            cause: new Error("This model's maximum context length is 128000 tokens."),
        });
        const detection = detectOverflow(wrapped);
        expect(detection.isOverflow).toBe(true);
        expect(detection.reportedLimit).toBe(128000);
    });

    test("finds overflow text in data.error.message under a generic wrapper message", () => {
        const detection = detectOverflow({
            name: "APICallError",
            message: "Bad Request",
            data: { error: { message: "input is too long for requested model" } },
        });
        expect(detection.isOverflow).toBe(true);
    });

    test("a cyclic error graph with no overflow text returns not-overflow", () => {
        const cyclic: Record<string, unknown> = { message: "Network error" };
        cyclic.cause = cyclic;
        cyclic.error = { data: cyclic };
        expect(detectOverflow(cyclic).isOverflow).toBe(false);
    });
});

describe("overflow-detection / parseReportedLimit", () => {
    test("extracts from 'maximum prompt length' (xAI)", () => {
        expect(parseReportedLimit("the maximum prompt length is 256000 tokens")).toEqual({
            value: 256000,
            provenance: "prompt_only",
        });
    });

    test("extracts from 'maximum context length' (OpenRouter/DeepSeek)", () => {
        expect(parseReportedLimit("maximum context length is 32768 tokens")).toEqual({
            value: 32768,
            provenance: "combined",
        });
    });

    test("extracts from 'context length is only' (vLLM)", () => {
        expect(parseReportedLimit("context length is only 4096 tokens")).toEqual({
            value: 4096,
            provenance: "combined",
        });
    });

    test("extracts from 'exceeds the limit of' (Copilot)", () => {
        expect(parseReportedLimit("Prompt exceeds the limit of 64000 tokens")).toEqual({
            value: 64000,
            provenance: "unknown",
        });
    });

    test("extracts Anthropic-style '> N maximum|max|limit' caps", () => {
        for (const suffix of ["maximum", "max", "limit"]) {
            expect(
                parseReportedLimit(`prompt is too long: 210000 tokens > 200000 ${suffix}`),
            ).toEqual({ value: 200000, provenance: "prompt_only" });
        }
    });

    test("extracts from 'too large for model with' (Mistral)", () => {
        expect(parseReportedLimit("Too large for model with 32768 maximum context length")).toEqual(
            { value: 32768, provenance: "combined" },
        );
    });

    test("rejects implausibly small numbers (< 1024)", () => {
        // Error codes like "413" should not be mistaken for context limits
        expect(parseReportedLimit("maximum context length is 100 tokens")).toBeUndefined();
    });

    test("rejects implausibly large numbers (> 10M)", () => {
        expect(parseReportedLimit("maximum context length is 999999999 tokens")).toBeUndefined();
    });

    test("returns undefined when no pattern matches", () => {
        expect(parseReportedLimit("Random error message")).toBeUndefined();
        expect(parseReportedLimit("")).toBeUndefined();
    });

    test("returns first plausible match when multiple numbers present", () => {
        const msg = "maximum context length is 128000 tokens (limit 999)";
        expect(parseReportedLimit(msg)).toEqual({ value: 128000, provenance: "combined" });
    });

    test("a limit stated before 'context' wins over a later request size", () => {
        expect(
            parseReportedLimit("maximum 128000 context length; request has 200000 tokens"),
        ).toEqual({ value: 128000, provenance: "unknown" });
    });
});

describe("overflow-detection / adversarial input cost", () => {
    // Provider error bodies are attacker-controlled; patterns must avoid superlinear backtracking.
    const MAX_MS = 100;

    function measure(message: string): number {
        const started = performance.now();
        detectOverflow(message);
        return performance.now() - started;
    }

    test("dense max/context repeats without digits stay linear (generic limit fallback)", () => {
        const message = `context_length_exceeded ${"max context ".repeat(600)}`;
        expect(measure(message)).toBeLessThan(MAX_MS);
    });

    test("dense 'input length ... exceeds' repeats stay linear (vLLM pattern)", () => {
        const message = "input length exceeds ".repeat(500);
        expect(measure(message)).toBeLessThan(MAX_MS);
    });

    test("dense 'input token count' repeats stay linear (Gemini pattern)", () => {
        const message = "input token count ".repeat(1500);
        expect(measure(message)).toBeLessThan(MAX_MS);
    });

    test("scan is capped so oversized bodies cost the same as capped ones", () => {
        const phrase = "prompt is too long: 210000 tokens > 200000 maximum";
        const within = detectOverflow(`${"x".repeat(MAX_SCAN_CHARS - phrase.length)}${phrase}`);
        expect(within.isOverflow).toBe(true);
        expect(within.reportedLimit).toBe(200000);

        const beyond = detectOverflow(`${"x".repeat(MAX_SCAN_CHARS)}${phrase}`);
        expect(beyond.isOverflow).toBe(false);

        expect(measure(`context_length_exceeded ${"max context ".repeat(50_000)}`)).toBeLessThan(
            MAX_MS,
        );
    });
});

describe("llama.cpp context-size limit extraction", () => {
    // The capture must include the full context-size limit because the plausibility clamp rejects single-digit captures.
    test("extracts the limit from llama.cpp-style messages", () => {
        expect(
            parseReportedLimit(
                "context size has been exceeded: limit 200000 tokens, you sent 214311",
            ),
        ).toMatchObject({ value: 200000, provenance: "combined" });
        expect(parseReportedLimit("context size exceeded: 128000 tokens maximum")).toMatchObject({
            value: 128000,
            provenance: "combined",
        });
    });

    test("does not capture a number more than 40 chars past the phrase", () => {
        // The anchor prevents distant numbers, such as request IDs, from binding.
        expect(
            parseReportedLimit(
                "context size problem occurred while handling the request submitted at position 99999999 tokens",
            ),
        ).toBeUndefined();
    });
});
