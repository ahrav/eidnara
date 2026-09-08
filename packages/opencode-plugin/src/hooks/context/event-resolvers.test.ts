import { describe, expect, it } from "bun:test";

import {
    resolveCacheTtl,
    resolveContextLimit,
    resolveExecuteThreshold,
    resolveExecuteThresholdDetail,
    resolveModelKey,
    resolveSessionId,
    resolveTrustedContextLimit,
} from "./event-resolvers";

describe("event-resolvers", () => {
    describe("resolveContextLimit", () => {
        // getModelsDevContextLimit overlays opencode.json provider limits on the models.dev cache.

        it("resolves anthropic context from models.dev when available", () => {
            const limit = resolveContextLimit("anthropic", "claude-opus-4-5");

            expect(limit).toBeLessThanOrEqual(200_000);
            expect(limit).toBeGreaterThan(0);
        });

        it("returns default for missing provider", () => {
            //#when
            const limit = resolveContextLimit(undefined, "gpt-4o");

            //#then
            expect(limit).toBe(128_000);
        });

        it("returns default for unknown provider/model not in models.dev or opencode.json", () => {
            //#when
            const limit = resolveContextLimit("unknown-provider", "unknown-model-xyz");

            //#then
            expect(limit).toBe(128_000);
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

        it("returns undefined for an unknown model (NOT the 128K default)", () => {
            expect(
                resolveTrustedContextLimit("unknown-provider", "unknown-model-xyz"),
            ).toBeUndefined();
        });

        it("returns undefined when provider/model missing", () => {
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

        it("caps any resolved value at 90%", () => {
            expect(resolveExecuteThreshold(95, "openai/gpt-4o", 65)).toBe(90);
            expect(
                resolveExecuteThreshold({ default: 95, "openai/gpt-4o": 90 }, "openai/gpt-4o", 65),
            ).toBe(90);
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

        it("prefers exact provider/model key when present", () => {
            const config = { default: 65, "openai/gpt-5.4-fast": 25 };

            //#when
            const result = resolveExecuteThreshold(config, "openai/gpt-5.4-fast", 65);

            //#then
            expect(result).toBe(25);
        });

        it("falls back to base model key when user wrote base (no derived)", () => {
            const config = { default: 65, "openai/gpt-5.4": 25 };

            const result = resolveExecuteThreshold(config, "openai/gpt-5.4-fast", 65);

            // Derived model keys match their base keys after suffix stripping.
            expect(result).toBe(25);
        });

        it("prefers most-specific match when both derived and base configured", () => {
            // When both keys exist, the derived key takes precedence.
            const config = {
                default: 65,
                "openai/gpt-5.4-fast": 20,
                "openai/gpt-5.4": 40,
            };

            //#when
            const derived = resolveExecuteThreshold(config, "openai/gpt-5.4-fast", 65);
            const base = resolveExecuteThreshold(config, "openai/gpt-5.4", 65);

            //#then
            expect(derived).toBe(20);
            expect(base).toBe(40);
        });

        it("matches bare model id (no provider prefix) in config", () => {
            // Providerless model IDs match configured model keys.
            const config = { default: 65, "gpt-5.4-fast": 25 };

            //#when
            const result = resolveExecuteThreshold(config, "openai/gpt-5.4-fast", 65);

            //#then
            expect(result).toBe(25);
        });

        it("matches bare base model id for derived runtime model", () => {
            //#given
            const config = { default: 65, "gpt-5.4": 30 };

            //#when
            const result = resolveExecuteThreshold(config, "openai/gpt-5.4-fast", 65);

            //#then
            expect(result).toBe(30);
        });

        it("returns config.default when no keys match", () => {
            //#given
            const config = { default: 55, "anthropic/claude-opus-4-6": 40 };

            //#when
            const result = resolveExecuteThreshold(config, "openai/gpt-4o", 65);

            //#then
            expect(result).toBe(55);
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

        it("matches the provider wildcard when no exact, base, or bare key matches", () => {
            const config = { default: 55, "anthropic/*": 70 };

            //#when
            const detail = resolveExecuteThresholdDetail(config, "anthropic/claude-opus-4-8", 65);

            //#then
            expect(detail.percentage).toBe(70);
            expect(detail.matchedKey).toBe("anthropic/*");
        });

        it("ranks the provider wildcard below exact, base, and bare keys but above default", () => {
            const runtimeKey = "anthropic/claude-opus-4-8";
            expect(
                resolveExecuteThreshold(
                    { default: 55, "anthropic/*": 70, "anthropic/claude-opus-4-8": 30 },
                    runtimeKey,
                    65,
                ),
            ).toBe(30);
            expect(
                resolveExecuteThreshold(
                    { default: 55, "anthropic/*": 70, "anthropic/claude-opus-4": 35 },
                    runtimeKey,
                    65,
                ),
            ).toBe(35);
            expect(
                resolveExecuteThreshold(
                    { default: 55, "anthropic/*": 70, "claude-opus-4-8": 40 },
                    runtimeKey,
                    65,
                ),
            ).toBe(40);
            expect(resolveExecuteThreshold({ default: 55, "openai/*": 70 }, runtimeKey, 65)).toBe(
                55,
            );
        });
    });

    describe("resolveExecuteThreshold (tokens-based)", () => {
        it("uses execute_threshold_tokens when set for the model, overriding percentage", () => {
            const result = resolveExecuteThreshold(65, "github-copilot/gpt-5.2-codex", 65, {
                tokensConfig: { "github-copilot/gpt-5.2-codex": 100_000 },
                contextLimit: 200_000,
            });

            expect(result).toBe(50);
        });

        it("uses execute_threshold_tokens.default for models not explicitly listed", () => {
            const result = resolveExecuteThreshold(65, "openai/gpt-5.4", 65, {
                tokensConfig: { default: 150_000 },
                contextLimit: 400_000,
            });

            //#then
            expect(result).toBe(37.5);
        });

        it("matches a provider wildcard in tokens config before falling to default", () => {
            const detail = resolveExecuteThresholdDetail(65, "anthropic/claude-opus-4-8", 65, {
                tokensConfig: { default: 150_000, "anthropic/*": 100_000 },
                contextLimit: 400_000,
            });

            //#then
            expect(detail.mode).toBe("tokens");
            expect(detail.matchedKey).toBe("anthropic/*");
            expect(detail.absoluteTokens).toBe(100_000);
            expect(detail.percentage).toBe(25);
        });

        it("clamps token value above 90% × contextLimit and still returns capped percentage", () => {
            const result = resolveExecuteThreshold(65, "some/model", 65, {
                tokensConfig: { "some/model": 500_000 },
                contextLimit: 200_000,
            });

            expect(result).toBe(90);
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

        it("falls through to percentage when contextLimit is missing (tokens unusable)", () => {
            // resolveExecuteThreshold ignores tokens when contextLimit is undefined.
            const result = resolveExecuteThreshold(55, "x/y", 65, {
                tokensConfig: { "x/y": 100_000 },
            });

            //#then
            expect(result).toBe(55);
        });

        it("picks exact model key before default in tokens config", () => {
            // An exact key takes precedence over the default.
            const result = resolveExecuteThreshold(65, "github-copilot/gpt-5.2-codex", 65, {
                tokensConfig: {
                    default: 200_000,
                    "github-copilot/gpt-5.2-codex": 40_000,
                },
                contextLimit: 400_000,
            });

            expect(result).toBe(10);
        });

        it("supports progressive lookup (derived → base) for tokens config", () => {
            // Derived variants use the base model's token setting.
            const result = resolveExecuteThreshold(65, "openai/gpt-5.4-fast", 65, {
                tokensConfig: { "openai/gpt-5.4": 100_000 },
                contextLimit: 400_000,
            });

            expect(result).toBe(25);
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

        it("reports mode='percentage' when contextLimit is missing (tokens unusable)", () => {
            // Without a contextLimit, tokens configuration cannot apply.
            const detail = resolveExecuteThresholdDetail(55, "x/y", 65, {
                tokensConfig: { "x/y": 100_000 },
            });

            //#then
            expect(detail.mode).toBe("percentage");
            expect(detail.percentage).toBe(55);
        });

        it("reports mode='tokens' with absoluteTokens equal to clamp cap when over-cap", () => {
            const detail = resolveExecuteThresholdDetail(65, "some/model", 65, {
                tokensConfig: { "some/model": 500_000 },
                contextLimit: 200_000,
                sessionId: "ses-test-clamp-detail",
            });

            // A valid tokens configuration takes precedence and clamps at 90%.
            expect(detail.mode).toBe("tokens");
            expect(detail.percentage).toBe(90);
            expect(detail.absoluteTokens).toBe(180_000);
        });

        it("guards against NaN contextLimit (runtime division hazard) — falls through to percentage", () => {
            const nanLimit = 0 / 0;

            //#when
            const detail = resolveExecuteThresholdDetail(55, "x/y", 65, {
                tokensConfig: { "x/y": 100_000 },
                contextLimit: nanLimit,
            });

            // NaN contextLimit falls back to percentage configuration.
            expect(detail.mode).toBe("percentage");
            expect(detail.percentage).toBe(55);
            expect(Number.isFinite(detail.percentage)).toBe(true);
        });

        it("guards against negative/zero contextLimit", () => {
            //#when
            const zero = resolveExecuteThresholdDetail(55, "x/y", 65, {
                tokensConfig: { "x/y": 100_000 },
                contextLimit: 0,
            });
            const neg = resolveExecuteThresholdDetail(55, "x/y", 65, {
                tokensConfig: { "x/y": 100_000 },
                contextLimit: -100_000,
            });

            // resolveExecuteThreshold ignores invalid context limits without throwing.
            expect(zero.mode).toBe("percentage");
            expect(neg.mode).toBe("percentage");
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

        it("dedupes clamp warn: repeated resolution of the same over-cap config only warns once", () => {
            // The resolver deduplicates clamp logs by (sessionId|modelKey|tokenVal|cap).
            const opts = {
                tokensConfig: { "some/model": 500_000 },
                contextLimit: 200_000,
                sessionId: "ses-dedupe",
            };
            const a = resolveExecuteThresholdDetail(65, "some/model", 65, opts);
            const b = resolveExecuteThresholdDetail(65, "some/model", 65, opts);
            const c = resolveExecuteThresholdDetail(65, "some/model", 65, opts);

            expect(a).toEqual(b);
            expect(b).toEqual(c);
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
