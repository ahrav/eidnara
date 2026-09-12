import { describe, expect, it } from "bun:test";
import { validateModelId } from "./model-picker";

describe("validateModelId", () => {
    it("accepts canonical provider/model ids, including ids that carry additional slashes", () => {
        expect(validateModelId("anthropic/claude-haiku-4-5")).toBeUndefined();
        expect(validateModelId("  openai/gpt-4o-mini  ")).toBeUndefined();
        expect(validateModelId("openrouter/anthropic/claude-3.5-haiku")).toBeUndefined();
    });

    it("rejects blank input", () => {
        expect(validateModelId("")).toBe("A model id is required");
        expect(validateModelId("   ")).toBe("A model id is required");
    });

    it("rejects a bare model name without a provider", () => {
        expect(validateModelId("claude-haiku")).toMatch(/provider\/model/);
    });

    it("rejects an empty provider or empty model segment", () => {
        expect(validateModelId("/claude-haiku")).toMatch(/provider\/model/);
        expect(validateModelId("anthropic/")).toMatch(/provider\/model/);
        expect(validateModelId("/")).toMatch(/provider\/model/);
    });

    it("rejects whitespace inside the id", () => {
        expect(validateModelId("openai / gpt-5")).toMatch(/without spaces/);
        expect(validateModelId("openai/ gpt-5")).toMatch(/without spaces/);
        expect(validateModelId("open ai/gpt-5")).toMatch(/without spaces/);
        expect(validateModelId("openai/gpt\t5")).toMatch(/without spaces/);
    });

    it("rejects NUL and other control characters inside the id", () => {
        expect(validateModelId("openai/gpt-5\u0000")).toMatch(/control characters/);
        expect(validateModelId("open\u0000ai/gpt-5")).toMatch(/control characters/);
        expect(validateModelId("openai/gpt\u001b5")).toMatch(/control characters/);
        expect(validateModelId("openai/gpt\u007f5")).toMatch(/control characters/);
    });

    it("rejects a provider or model segment that starts with a dash", () => {
        expect(validateModelId("-openai/gpt-5")).toMatch(/start with '-'/);
        expect(validateModelId("openai/-gpt-5")).toMatch(/start with '-'/);
        expect(validateModelId("openai/gpt-5-mini")).toBeUndefined();
    });

    it("rejects a provider or model segment over 256 UTF-8 bytes", () => {
        const emoji = "🙂".repeat(100); // 400 bytes, 200 UTF-16 code units
        expect(validateModelId(`p/${emoji}`)).toMatch(/at most 256 bytes/);
        expect(validateModelId(`${emoji}/m`)).toMatch(/at most 256 bytes/);
        expect(validateModelId(`p/${"m".repeat(256)}`)).toBeUndefined();
        expect(validateModelId(`p/${"m".repeat(257)}`)).toMatch(/at most 256 bytes/);
    });
});
