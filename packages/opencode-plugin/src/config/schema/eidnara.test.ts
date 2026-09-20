import { describe, expect, it } from "bun:test";
import {
    assertKnownConfigKeys,
    DEFAULT_HISTORY_BUDGET_PERCENTAGE,
    DEFAULT_HISTORY_SUMMARIZER_TIMEOUT_MS,
    dropRemovedConfigKeys,
    type EidnaraConfig,
    EidnaraConfigSchema,
    REMOVED_CONFIG_KEYS,
} from "./eidnara";

describe("EidnaraConfigSchema", () => {
    describe("defaults", () => {
        it("applies defaults for an empty config", () => {
            const result = EidnaraConfigSchema.parse({});

            expect(result).toMatchObject({
                enabled: true,
                allow_home_project: false,
                fail_closed_blocking: true,
                storage: { enforce_private_permissions: true },
                cache_ttl: "5m",
                prompt_surface: { default: "full" },
                execute_threshold_percentage: 65,
                protected_tags: 20,
                clear_reasoning_age: 50,
                history_budget_percentage: DEFAULT_HISTORY_BUDGET_PERCENTAGE,
                history_summarizer_timeout_ms: DEFAULT_HISTORY_SUMMARIZER_TIMEOUT_MS,
                memory: {
                    enabled: true,
                    injection_budget_tokens: 4000,
                    auto_promote: true,
                    retrieval_count_promotion_threshold: 3,
                },
                todowrite: {
                    enabled: true,
                    overlay: true,
                },
            });
            expect(result.history_summarizer).toBeUndefined();
            expect(result.context_researcher).toBeUndefined();
            expect(result.pi).toBeUndefined();
        });
    });

    describe("budget configuration", () => {
        it("accepts 90% execute thresholds and rejects 91%", () => {
            expect(
                EidnaraConfigSchema.safeParse({ execute_threshold_percentage: 90 }).success,
            ).toBe(true);
            expect(
                EidnaraConfigSchema.safeParse({ execute_threshold_percentage: 91 }).success,
            ).toBe(false);
        });

        it("accepts numeric and per-model output reserves including zero", () => {
            expect(EidnaraConfigSchema.parse({ output_reserve: 0 }).output_reserve).toBe(0);
            expect(
                EidnaraConfigSchema.parse({
                    output_reserve: { default: 16_384, "google/gemini": 0 },
                }).output_reserve,
            ).toEqual({ default: 16_384, "google/gemini": 0 });
            expect(EidnaraConfigSchema.safeParse({ output_reserve: -1 }).success).toBe(false);
        });
    });

    describe("valid config", () => {
        it("parses an enabled config without stale reduction-specific keys", () => {
            const input = {
                enabled: true,
                allow_home_project: false,
                fail_closed_blocking: true,
                toast_duration_ms: 5000,
                cache_ttl: "10m",
                prompt_surface: { default: "full" },
                protected_tags: 3,
                execute_threshold_percentage: 75,
                clear_reasoning_age: 60,
                history_budget_percentage: 0.2,
                history_summarizer_timeout_ms: 360_000,
                commit_cluster_trigger: {
                    enabled: true,
                    min_clusters: 3,
                },
                sqlite: {
                    cache_size_mb: 64,
                    mmap_size_mb: 0,
                },
                storage: {
                    enforce_private_permissions: false,
                },
                system_prompt_injection: {
                    enabled: true,
                    skip_signatures: ["<!-- eidnara: skip -->"],
                },
                temporal_awareness: false,
                keep_subagents: false,
                todowrite: {
                    enabled: false,
                    overlay: false,
                },
                smart_drops: false,
                shadow_embedding: {
                    enabled: false,
                },
                terse_text_compression: {
                    enabled: false,
                    min_chars: 500,
                },
                memory: {
                    enabled: true,
                    injection_budget_tokens: 4000,
                    auto_promote: true,
                    auto_capture: true,
                    retrieval_count_promotion_threshold: 3,
                    auto_search: {
                        enabled: false,
                        score_threshold: 0.6,
                        min_prompt_chars: 20,
                    },
                    git_commit_indexing: {
                        enabled: false,
                        since_days: 365,
                        max_commits: 2000,
                    },
                },
                pi: {
                    subagent_extensions: ["@example/provider", "./extensions/local.ts"],
                },
                context_researcher: {
                    disable: false,
                    model: "qwen-test",
                    fallback_models: ["qwen-fallback"],
                    temperature: 0.1,
                    variant: "fast",
                    timeout_ms: 12_000,
                    system_prompt: "Custom prompt",
                },
                compaction: {
                    enabled: true,
                },
            } satisfies EidnaraConfig;

            const result = EidnaraConfigSchema.parse(input);

            expect(result).toEqual(input);
        });

        it("accepts a boolean storage permission policy and rejects non-booleans", () => {
            expect(
                EidnaraConfigSchema.parse({
                    storage: { enforce_private_permissions: false },
                }).storage.enforce_private_permissions,
            ).toBe(false);
            expect(
                EidnaraConfigSchema.safeParse({
                    storage: { enforce_private_permissions: "false" },
                }).success,
            ).toBe(false);
        });

        it("applies context_researcher defaults when the object is present", () => {
            const result = EidnaraConfigSchema.parse({
                context_researcher: {
                    model: "github-copilot/gpt-5.4",
                },
            });

            expect(result.context_researcher).toEqual({
                model: "github-copilot/gpt-5.4",
                timeout_ms: 30000,
            });
        });

        it("accepts disable on hidden agents without deprecated enabled", () => {
            const result = EidnaraConfigSchema.parse({
                history_summarizer: { disable: true },
                context_researcher: { disable: true },
            });

            expect(result.history_summarizer?.disable).toBe(true);
            expect(result.context_researcher?.disable).toBe(true);
            expect("enabled" in (result.context_researcher as Record<string, unknown>)).toBe(false);
        });

        it("has no transform_mode: the daemon transform is the only transform", () => {
            expect("transform_mode" in EidnaraConfigSchema.parse({})).toBe(false);
            expect(() => assertKnownConfigKeys({ transform_mode: "rust" })).toThrow(
                /Unknown Eidnara configuration key/,
            );
            const raw: Record<string, unknown> = { transform_mode: "ts", enabled: true };
            expect(dropRemovedConfigKeys(raw)).toEqual([REMOVED_CONFIG_KEYS.transform_mode]);
            expect(raw).toEqual({ enabled: true });
            expect(dropRemovedConfigKeys(raw)).toEqual([]);
        });

        it("rejects removed configuration keys", () => {
            for (const key of [
                "memory_classifier",
                "embedding",
                "auto_update",
                "mural",
                "conditional_notes",
            ]) {
                expect(EidnaraConfigSchema.safeParse({ [key]: {} }).success).toBe(false);
            }
        });

        it("fills the default for a per-model percentage map that omits it", () => {
            expect(
                EidnaraConfigSchema.parse({ execute_threshold_percentage: { "openai/gpt-4": 80 } })
                    .execute_threshold_percentage,
            ).toEqual({ default: 65, "openai/gpt-4": 80 });
            expect(
                EidnaraConfigSchema.parse({
                    execute_threshold_percentage: { default: 70, "openai/gpt-4": 80 },
                }).execute_threshold_percentage,
            ).toEqual({ default: 70, "openai/gpt-4": 80 });
        });

        it("accepts and normalizes 2-letter ISO 639-1 language codes", () => {
            expect(EidnaraConfigSchema.parse({ language: "tr" }).language).toBe("tr");
            expect(EidnaraConfigSchema.parse({ language: "  ES " }).language).toBe("es");
            expect(EidnaraConfigSchema.parse({ language: "ja" }).language).toBe("ja");
        });

        it("rejects language values that are not two letters or do not name a language", () => {
            for (const language of ["english", "Turkish", "tur", "e", "t1", "<x>", "", "zz"]) {
                const result = EidnaraConfigSchema.safeParse({ language });
                expect([language, result.success]).toEqual([language, false]);
                // The shape check aborts before the ISO lookup so a malformed value reports one issue.
                expect([language, result.error?.issues.length]).toEqual([language, 1]);
            }
        });

        it("parses per-model cache_ttl objects", () => {
            const input = {
                cache_ttl: {
                    default: "5m",
                    "claude-3-haiku": "10m",
                    "gpt-4": "2m",
                },
            };

            const result = EidnaraConfigSchema.parse(input);

            expect(result.cache_ttl).toEqual(input.cache_ttl);
        });

        it("parses prompt-surface defaults, routes, and user overrides", () => {
            const promptSurface = {
                default: "light" as const,
                models: {
                    "anthropic/claude/sonnet": "full" as const,
                    "claude-sonnet-4-5": "light" as const,
                    "openai/*": "light" as const,
                },
                guidance_override_path: "./guidance.md",
                tool_descriptions: { eidnara_search: "Search project context" },
            };

            expect(
                EidnaraConfigSchema.parse({ prompt_surface: promptSurface }).prompt_surface,
            ).toEqual(promptSurface);
        });
    });

    describe("validation", () => {
        it("rejects malformed prompt-surface model keys and empty override text", () => {
            const malformedKeys = [
                "",
                "model*",
                "/model",
                "provider/",
                "provider//model",
                "provider/model*",
                "provider/*/nested",
                "provider//model",
                "provider/model/",
                "provider/ model",
                "provider /model",
                " provider/model",
                "provider/model ",
                "*/model",
                "*",
                "\u00a0model",
            ];
            for (const key of malformedKeys) {
                expect([
                    key,
                    EidnaraConfigSchema.safeParse({
                        prompt_surface: { models: { [key]: "light" } },
                    }).success,
                ]).toEqual([key, false]);
            }

            const acceptedKeys = ["gpt-4", "openai/gpt-4", "openai/*", "openai/gpt/4/x", "a b/c d"];
            for (const key of acceptedKeys) {
                expect([
                    key,
                    EidnaraConfigSchema.safeParse({
                        prompt_surface: { models: { [key]: "light" } },
                    }).success,
                ]).toEqual([key, true]);
            }

            expect(
                EidnaraConfigSchema.safeParse({
                    prompt_surface: { guidance_override_path: "  " },
                }).success,
            ).toBe(false);
            expect(
                EidnaraConfigSchema.safeParse({
                    prompt_surface: { tool_descriptions: { eidnara_search: "  " } },
                }).success,
            ).toBe(false);
            expect(
                EidnaraConfigSchema.safeParse({
                    prompt_surface: { tool_descriptions: { "  ": "description" } },
                }).success,
            ).toBe(false);
        });

        it("rejects whitespace-only trimmed path, model, and Pi extension fields but keeps trimming valid ones", () => {
            const blank = [
                { models: { window_overlay_path: "\t" } },
                { host: { connection_file: " " } },
                { pi: { subagent_extensions: ["  "] } },
            ];
            for (const input of blank) {
                expect([input, EidnaraConfigSchema.safeParse(input).success]).toEqual([
                    input,
                    false,
                ]);
            }

            expect(
                EidnaraConfigSchema.parse({ models: { window_overlay_path: " ./o.json " } }).models
                    ?.window_overlay_path,
            ).toBe("./o.json");
            expect(
                EidnaraConfigSchema.parse({ pi: { subagent_extensions: [" ext "] } }).pi
                    ?.subagent_extensions,
            ).toEqual(["ext"]);
        });

        it("accepts protected_tags at both bounds and rejects numeric fields outside their schema ranges", () => {
            expect(EidnaraConfigSchema.parse({ protected_tags: 1 }).protected_tags).toBe(1);
            expect(EidnaraConfigSchema.parse({ protected_tags: 20 }).protected_tags).toBe(20);
            expect(EidnaraConfigSchema.parse({ protected_tags: 100 }).protected_tags).toBe(100);

            const outOfRange = [
                { protected_tags: 0 },
                { protected_tags: 101 },
                { clear_reasoning_age: 9 },
                { history_summarizer_timeout_ms: 59_999 },
            ];
            for (const input of outOfRange) {
                expect([input, EidnaraConfigSchema.safeParse(input).success]).toEqual([
                    input,
                    false,
                ]);
            }
        });
    });
});
