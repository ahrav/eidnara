import { describe, expect, test } from "bun:test";

import { modelBodyField, parseProviderModel, resolveFallbackChain } from "./resolve-fallbacks";

describe("resolveFallbackChain", () => {
    // resolveFallbackChain has no built-in provider-agnostic fallback chain.
    test("returns empty for undefined, an empty string, and an empty array (no builtin chain)", () => {
        expect(resolveFallbackChain(undefined)).toEqual([]);
        expect(resolveFallbackChain("")).toEqual([]);
        expect(resolveFallbackChain([])).toEqual([]);
    });

    test("user-only when user provides a valid fallback_models string or array", () => {
        expect(resolveFallbackChain("anthropic/claude-sonnet-4-6")).toEqual([
            "anthropic/claude-sonnet-4-6",
        ]);
        expect(
            resolveFallbackChain(["anthropic/claude-sonnet-4-6", "google/gemini-3-flash"]),
        ).toEqual(["anthropic/claude-sonnet-4-6", "google/gemini-3-flash"]);
    });

    test("dedupes user-provided list", () => {
        expect(
            resolveFallbackChain([
                "anthropic/claude-sonnet-4-6",
                "anthropic/claude-sonnet-4-6",
                "google/gemini-3-flash",
            ]),
        ).toEqual(["anthropic/claude-sonnet-4-6", "google/gemini-3-flash"]);
    });

    test("strips invalid 'provider/model' entries", () => {
        expect(
            resolveFallbackChain([
                "anthropic/claude-sonnet-4-6",
                "no-slash-here",
                "/leading-slash",
                "trailing-slash/",
                "",
                "  ",
            ]),
        ).toEqual(["anthropic/claude-sonnet-4-6"]);
    });

    test("trims whitespace in user entries", () => {
        expect(
            resolveFallbackChain(["  anthropic/claude-sonnet-4-6  ", "\tgoogle/gemini-3-flash\n"]),
        ).toEqual(["anthropic/claude-sonnet-4-6", "google/gemini-3-flash"]);
    });

    test("dedupes entries that differ only by whitespace around the slash", () => {
        expect(
            resolveFallbackChain([
                "anthropic/claude-sonnet-4-6",
                "anthropic / claude-sonnet-4-6",
                "anthropic/ claude-sonnet-4-6",
                "lemonade/GLM-4.7-Flash-GGUF/main",
                "lemonade /GLM-4.7-Flash-GGUF/main",
            ]),
        ).toEqual(["anthropic/claude-sonnet-4-6", "lemonade/GLM-4.7-Flash-GGUF/main"]);
    });
});

describe("parseProviderModel", () => {
    test("splits on the first slash and trims surrounding whitespace", () => {
        expect(parseProviderModel("anthropic/claude-sonnet-4-6")).toEqual({
            providerID: "anthropic",
            modelID: "claude-sonnet-4-6",
        });
        expect(parseProviderModel("lemonade/GLM-4.7-Flash-GGUF/main")).toEqual({
            providerID: "lemonade",
            modelID: "GLM-4.7-Flash-GGUF/main",
        });
        expect(parseProviderModel("  anthropic/claude-sonnet-4-6  ")).toEqual({
            providerID: "anthropic",
            modelID: "claude-sonnet-4-6",
        });
    });

    test("returns null without a slash, with an empty side, or when whitespace is the only text beside the slash", () => {
        for (const input of [
            "anthropic",
            "/claude-sonnet-4-6",
            "anthropic/",
            "",
            " /claude-sonnet-4-6",
            "anthropic/ ",
            " / ",
        ]) {
            expect(parseProviderModel(input), JSON.stringify(input)).toBeNull();
        }
    });
});

describe("modelBodyField", () => {
    test("omits the model field when a part is whitespace-only", () => {
        expect(modelBodyField("anthropic/ ")).toEqual({});
        expect(modelBodyField(" /claude-sonnet-4-6")).toEqual({});
    });

    test("carries a trimmed provider/model pair", () => {
        expect(modelBodyField(" anthropic/claude-sonnet-4-6 ")).toEqual({
            model: { providerID: "anthropic", modelID: "claude-sonnet-4-6" },
        });
    });
});
