import { describe, expect, it } from "bun:test";

import {
    parseCacheTtlMs,
    resolveCacheTtl,
    resolveContextLimit,
    resolveExecuteThreshold,
    resolveExecuteThresholdDetail,
    resolveModelKey,
    resolveSessionId,
    resolveTrustedContextLimit,
} from "./event-resolvers";

describe("event-resolvers", () => {
    describe("parseCacheTtlMs", () => {
        it("follows the daemon grammar: bare ms, s/m/h units, never, and rejects the rest", () => {
            expect(parseCacheTtlMs("1500")).toBe(1_500);
            expect(parseCacheTtlMs("30s")).toBe(30_000);
            expect(parseCacheTtlMs("5m")).toBe(300_000);
            expect(parseCacheTtlMs(" 1h ")).toBe(3_600_000);
            expect(parseCacheTtlMs("never")).toBe(Number.POSITIVE_INFINITY);
            expect(parseCacheTtlMs("NEVER")).toBe(Number.POSITIVE_INFINITY);
            expect(parseCacheTtlMs("")).toBeUndefined();
            expect(parseCacheTtlMs("5d")).toBeUndefined();
            expect(parseCacheTtlMs("1.5m")).toBeUndefined();
            expect(parseCacheTtlMs("m")).toBeUndefined();
        });
    });

    describe("resolveContextLimit", () => {
        // getModelsDevContextLimit overlays opencode.json provider limits on the models.dev cache.

        it("resolves anthropic context from models.dev when available", () => {
            const limit = resolveContextLimit("anthropic", "claude-opus-4-5");

            expect(limit).toBeLessThanOrEqual(200_000);
            expect(limit).toBeGreaterThan(0);
        });

        it("returns the 128K default for a missing provider or an unknown provider/model", () => {
            expect(resolveContextLimit(undefined, "gpt-4o")).toBe(128_000);
            expect(resolveContextLimit("unknown-provider", "unknown-model-xyz")).toBe(128_000);
        });
    });

    describe("resolveTrustedContextLimit", () => {
        // resolveTrustedContextLimit prevents unknown models from shrinking history budgets based on the 128K fallback.
        // resolveTrustedContextLimit trusts only models.dev limits.

        it("returns a real limit for a known model (not undefined)", () => {
            const limit = resolveTrustedContextLimit("anthropic", "claude-opus-4-5");
            // Without models.json, resolveTrustedContextLimit returns undefined rather than the 128K fallback.
            if (limit !== undefined) {
                expect(limit).toBeGreaterThan(0);
                expect(limit).not.toBe(128_000);
            }
        });

        it("returns undefined, never the 128K default, for an unknown model or a missing provider/model", () => {
            expect(
                resolveTrustedContextLimit("unknown-provider", "unknown-model-xyz"),
            ).toBeUndefined();
            expect(resolveTrustedContextLimit(undefined, "gpt-4o")).toBeUndefined();
            expect(resolveTrustedContextLimit("anthropic", undefined)).toBeUndefined();
        });
    });

    describe("resolveCacheTtl", () => {
        it("returns direct string ttl for string config", () => {
            //#when
            const ttl = resolveCacheTtl("5m", "openai/gpt-4o");

            //#then
            expect(ttl).toBe("5m");
        });

        it("accepts Pi-native keys when the runtime model key is canonical", () => {
            expect(
                resolveCacheTtl(
                    { default: "5m", "openai-codex/gpt-5.6-sol": "60m" },
                    "openai/gpt-5.6-sol",
                ),
            ).toBe("60m");
        });

        it("resolves provider/model and bare-model overrides", () => {
            //#given
            const cacheTtl = {
                default: "5m",
                "openai/gpt-4o": "1m",
                "gpt-4o-mini": "2m",
            };

            //#when
            const providerModel = resolveCacheTtl(cacheTtl, "openai/gpt-4o");
            const bareModel = resolveCacheTtl(cacheTtl, "openai/gpt-4o-mini");

            //#then
            expect(providerModel).toBe("1m");
            expect(bareModel).toBe("2m");
        });
    });

    describe("resolveExecuteThreshold", () => {
        it("returns direct number config unchanged (after max cap)", () => {
            expect(resolveExecuteThreshold(50, "openai/gpt-5.4-fast", 65)).toBe(50);
            expect(resolveExecuteThreshold(50, undefined, 65)).toBe(50);
        });

        it("accepts Pi-native threshold keys and keeps canonical precedence", () => {
            expect(
                resolveExecuteThreshold(
                    { default: 65, "openai-codex/gpt-5.6-sol": 40 },
                    "openai/gpt-5.6-sol",
                    65,
                ),
            ).toBe(40);
            expect(
                resolveExecuteThreshold(
                    {
                        default: 65,
                        "openai-codex/gpt-5.6-sol": 40,
                        "openai/gpt-5.6-sol": 30,
                    },
                    "openai-codex/gpt-5.6-sol",
                    65,
                ),
            ).toBe(30);
        });

        it("resolves record keys in lookup order: exact, base, bare, bare base, provider wildcard, then default", () => {
            const opus = "anthropic/claude-opus-4-8";
            const cases: Array<{
                config: { default: number; [key: string]: number };
                modelKey: string;
                percentage: number;
                matchedKey: string;
            }> = [
                {
                    config: { default: 65, "openai/gpt-5.4-fast": 25 },
                    modelKey: "openai/gpt-5.4-fast",
                    percentage: 25,
                    matchedKey: "openai/gpt-5.4-fast",
                },
                {
                    config: { default: 65, "openai/gpt-5.4": 25 },
                    modelKey: "openai/gpt-5.4-fast",
                    percentage: 25,
                    matchedKey: "openai/gpt-5.4",
                },
                {
                    config: { default: 65, "openai/gpt-5.4-fast": 20, "openai/gpt-5.4": 40 },
                    modelKey: "openai/gpt-5.4-fast",
                    percentage: 20,
                    matchedKey: "openai/gpt-5.4-fast",
                },
                {
                    config: { default: 65, "openai/gpt-5.4-fast": 20, "openai/gpt-5.4": 40 },
                    modelKey: "openai/gpt-5.4",
                    percentage: 40,
                    matchedKey: "openai/gpt-5.4",
                },
                {
                    config: { default: 65, "gpt-5.4-fast": 25 },
                    modelKey: "openai/gpt-5.4-fast",
                    percentage: 25,
                    matchedKey: "gpt-5.4-fast",
                },
                {
                    config: { default: 65, "gpt-5.4": 30 },
                    modelKey: "openai/gpt-5.4-fast",
                    percentage: 30,
                    matchedKey: "gpt-5.4",
                },
                {
                    config: { default: 55, "anthropic/*": 70, [opus]: 30 },
                    modelKey: opus,
                    percentage: 30,
                    matchedKey: opus,
                },
                {
                    config: { default: 55, "anthropic/*": 70, "anthropic/claude-opus-4": 35 },
                    modelKey: opus,
                    percentage: 35,
                    matchedKey: "anthropic/claude-opus-4",
                },
                {
                    config: { default: 55, "anthropic/*": 70, "claude-opus-4-8": 40 },
                    modelKey: opus,
                    percentage: 40,
                    matchedKey: "claude-opus-4-8",
                },
                {
                    config: { default: 55, "anthropic/*": 70 },
                    modelKey: opus,
                    percentage: 70,
                    matchedKey: "anthropic/*",
                },
                {
                    config: { default: 55, "anthropic/claude-opus-4-6": 40 },
                    modelKey: "openai/gpt-4o",
                    percentage: 55,
                    matchedKey: "default",
                },
                {
                    config: { default: 55, "openai/*": 70 },
                    modelKey: opus,
                    percentage: 55,
                    matchedKey: "default",
                },
            ];

            for (const { config, modelKey, percentage, matchedKey } of cases) {
                const label = `${modelKey} against ${JSON.stringify(config)}`;
                const detail = resolveExecuteThresholdDetail(config, modelKey, 65);
                expect(detail.percentage, label).toBe(percentage);
                expect(detail.matchedKey, label).toBe(matchedKey);
                expect(resolveExecuteThreshold(config, modelKey, 65), label).toBe(percentage);
            }
        });

        it("returns fallback when config.default absent and no match", () => {
            //#given
            const config = { default: 0, "anthropic/claude-opus-4-6": 40 } as unknown as {
                default: number;
                [key: string]: number;
            };
            delete (config as Record<string, unknown>).default;

            //#when
            const result = resolveExecuteThreshold(
                config as { default: number; [key: string]: number },
                "openai/gpt-4o",
                65,
            );

            //#then
            expect(result).toBe(65);
        });

        it("returns config.default when modelKey is undefined", () => {
            //#given
            const config = { default: 42, "openai/gpt-5.4-fast": 25 };

            //#when
            const result = resolveExecuteThreshold(config, undefined, 65);

            expect(result).toBe(42);
        });
    });

    describe("resolveExecuteThreshold (tokens-based)", () => {
        it("resolves tokens keys in lookup order: exact key, provider wildcard, then default", () => {
            const cases: Array<{
                tokensConfig: { default?: number; [key: string]: number | undefined };
                modelKey: string;
                contextLimit: number;
                absoluteTokens: number;
                percentage: number;
                matchedKey: string;
            }> = [
                {
                    tokensConfig: { default: 200_000, "github-copilot/gpt-5.2-codex": 40_000 },
                    modelKey: "github-copilot/gpt-5.2-codex",
                    contextLimit: 400_000,
                    absoluteTokens: 40_000,
                    percentage: 10,
                    matchedKey: "github-copilot/gpt-5.2-codex",
                },
                {
                    tokensConfig: { default: 150_000, "anthropic/*": 100_000 },
                    modelKey: "anthropic/claude-opus-4-8",
                    contextLimit: 400_000,
                    absoluteTokens: 100_000,
                    percentage: 25,
                    matchedKey: "anthropic/*",
                },
                {
                    tokensConfig: { default: 150_000 },
                    modelKey: "openai/gpt-5.4",
                    contextLimit: 400_000,
                    absoluteTokens: 150_000,
                    percentage: 37.5,
                    matchedKey: "default",
                },
            ];

            for (const { tokensConfig, modelKey, contextLimit, ...expected } of cases) {
                const label = `${modelKey} against ${JSON.stringify(tokensConfig)}`;
                const detail = resolveExecuteThresholdDetail(65, modelKey, 65, {
                    tokensConfig,
                    contextLimit,
                });
                expect(detail, label).toMatchObject({ mode: "tokens", ...expected });
                expect(
                    resolveExecuteThreshold(65, modelKey, 65, { tokensConfig, contextLimit }),
                    label,
                ).toBe(expected.percentage);
            }
        });

        it("falls through to percentage config when tokens config is missing", () => {
            //#when
            const result = resolveExecuteThreshold(
                { default: 60, "openai/gpt-5.4": 45 },
                "openai/gpt-5.4",
                65,
                { tokensConfig: undefined, contextLimit: 400_000 },
            );

            expect(result).toBe(45);
        });
    });

    describe("resolveExecuteThresholdDetail (mode + hardening)", () => {
        it("reports mode='tokens' and absoluteTokens when tokens match (exact)", () => {
            //#when
            const detail = resolveExecuteThresholdDetail(65, "github-copilot/gpt-5.2-codex", 65, {
                tokensConfig: { "github-copilot/gpt-5.2-codex": 100_000 },
                contextLimit: 200_000,
            });

            expect(detail.mode).toBe("tokens");
            expect(detail.percentage).toBe(50);
            expect(detail.absoluteTokens).toBe(100_000);
            expect(detail.matchedKey).toBe("github-copilot/gpt-5.2-codex");
        });

        it("reports mode='tokens' via progressive base-model match (display-drift fix)", () => {
            // A base-model tokens key must match a derived runtime model.
            const detail = resolveExecuteThresholdDetail(65, "openai/gpt-5.4-fast", 65, {
                tokensConfig: { "openai/gpt-5.4": 100_000 },
                contextLimit: 400_000,
            });

            // A matching base key selects tokens mode.
            expect(detail.mode).toBe("tokens");
            expect(detail.percentage).toBe(25);
            expect(detail.absoluteTokens).toBe(100_000);
            expect(detail.matchedKey).toBe("openai/gpt-5.4");
        });

        it("reports mode='percentage' when no tokens key or default matches", () => {
            const detail = resolveExecuteThresholdDetail(
                { default: 55, "openai/gpt-5.4": 45 },
                "openai/gpt-5.4",
                65,
                {
                    tokensConfig: { "other/model": 100_000 },
                    contextLimit: 400_000,
                },
            );

            //#then
            expect(detail.mode).toBe("percentage");
            expect(detail.percentage).toBe(45);
            expect(detail.absoluteTokens).toBeUndefined();
            expect(detail.matchedKey).toBe("openai/gpt-5.4");
        });

        it("reports mode='percentage' when contextLimit is missing, NaN, zero, or negative (tokens unusable)", () => {
            for (const contextLimit of [undefined, 0 / 0, 0, -100_000]) {
                const detail = resolveExecuteThresholdDetail(55, "x/y", 65, {
                    tokensConfig: { "x/y": 100_000 },
                    contextLimit,
                });

                expect(detail.mode, `contextLimit=${contextLimit}`).toBe("percentage");
                expect(detail.percentage, `contextLimit=${contextLimit}`).toBe(55);
                expect(Number.isFinite(detail.percentage)).toBe(true);
            }
        });

        it("guards against non-finite/non-positive token values (e.g., NaN injected at runtime)", () => {
            // A NaN token value falls back to percentage configuration.
            const detail = resolveExecuteThresholdDetail(55, "x/y", 65, {
                tokensConfig: { "x/y": Number.NaN },
                contextLimit: 200_000,
            });

            expect(detail.mode).toBe("percentage");
            expect(detail.percentage).toBe(55);
        });

        it("guards against negative percentage config by reverting to fallback", () => {
            //#when
            const detail = resolveExecuteThresholdDetail(-5 as unknown as number, "x/y", 42);

            // Negative percentage values use the fallback percentage.
            expect(detail.mode).toBe("percentage");
            expect(detail.percentage).toBe(42);
        });

        it("sets clamped + configuredValue when a tokens config is reduced to the cap (#241)", () => {
            const detail = resolveExecuteThresholdDetail(65, "some/model", 65, {
                tokensConfig: { "some/model": 190_000 },
                contextLimit: 128_000,
                sessionId: "ses-test-clamped-flag-tokens",
            });

            // Clamp metadata preserves the requested token value for displays.
            expect(detail.mode).toBe("tokens");
            expect(detail.clamped).toBe(true);
            expect(detail.configuredValue).toBe(190_000);
            expect(detail.absoluteTokens).toBe(115_200);
            expect(detail.percentage).toBe(90);
        });

        it("leaves clamped unset when a tokens config fits under the cap (#241)", () => {
            const detail = resolveExecuteThresholdDetail(65, "some/model", 65, {
                tokensConfig: { "some/model": 100_000 },
                contextLimit: 200_000,
            });

            expect(detail.mode).toBe("tokens");
            expect(detail.clamped).toBeUndefined();
            expect(detail.configuredValue).toBeUndefined();
            expect(detail.absoluteTokens).toBe(100_000);
        });

        it("sets clamped + configuredValue when a percentage config is capped at 90 (#241)", () => {
            const detail = resolveExecuteThresholdDetail(95, "some/model", 65);

            // The resolver caps the threshold at 90% and retains 95% for display.
            expect(detail.mode).toBe("percentage");
            expect(detail.clamped).toBe(true);
            expect(detail.configuredValue).toBe(95);
            expect(detail.percentage).toBe(90);
        });

        it("leaves clamped unset for an in-range percentage config (#241)", () => {
            //#when
            const detail = resolveExecuteThresholdDetail(65, "some/model", 65);

            //#then
            expect(detail.mode).toBe("percentage");
            expect(detail.clamped).toBeUndefined();
            expect(detail.configuredValue).toBeUndefined();
            expect(detail.percentage).toBe(65);
        });
    });

    describe("resolveModelKey", () => {
        it("returns the canonical provider/model key when both parts exist", () => {
            expect(resolveModelKey("openai", "gpt-4o")).toBe("openai/gpt-4o");
            expect(resolveModelKey("openai-codex", "gpt-5.6-sol")).toBe("openai/gpt-5.6-sol");
        });

        it("returns undefined when either part is missing", () => {
            expect(resolveModelKey(undefined, "gpt-4o")).toBeUndefined();
            expect(resolveModelKey("openai", undefined)).toBeUndefined();
        });
    });

    describe("resolveSessionId", () => {
        it("prefers properties.sessionID when present", () => {
            const sessionId = resolveSessionId({
                sessionID: "ses-direct",
                info: { id: "ses-info" },
            });
            expect(sessionId).toBe("ses-direct");
        });

        it("falls back to info.sessionID and info.id", () => {
            expect(resolveSessionId({ info: { sessionID: "ses-info" } })).toBe("ses-info");
            expect(resolveSessionId({ info: { id: "ses-id" } })).toBe("ses-id");
        });
    });
});
