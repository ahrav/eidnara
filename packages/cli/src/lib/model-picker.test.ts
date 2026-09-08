import { describe, expect, it } from "bun:test";
import { validateModelId } from "./model-picker";

describe("validateModelId", () => {
    it("accepts canonical provider/model ids", () => {
        expect(validateModelId("anthropic/claude-haiku-4-5")).toBeUndefined();
        expect(validateModelId("  openai/gpt-4o-mini  ")).toBeUndefined();
    });

    it("accepts model ids that carry additional slashes", () => {
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
});
