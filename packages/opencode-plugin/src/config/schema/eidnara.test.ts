import { describe, expect, it } from "bun:test";
import {
    DEFAULT_HISTORIAN_TIMEOUT_MS,
    DEFAULT_HISTORY_BUDGET_PERCENTAGE,
    DEFAULT_LOCAL_EMBEDDING_MODEL,
    type EidnaraConfig,
    EidnaraConfigSchema,
} from "./eidnara";

describe("EidnaraConfigSchema", () => {
    describe("defaults", () => {
        it("applies defaults for an empty config", () => {
            const result = EidnaraConfigSchema.parse({});

            expect(result).toMatchObject({
                enabled: true,
                allow_home_project: false,
                fail_closed_blocking: true,
                transform_mode: "ts",
                storage: { enforce_private_permissions: true },
                smart_notes: { retina_handoff: false },
                cache_ttl: "5m",
                prompt_surface: { default: "full" },
                execute_threshold_percentage: 65,
                protected_tags: 20,
                clear_reasoning_age: 50,
                history_budget_percentage: DEFAULT_HISTORY_BUDGET_PERCENTAGE,
                historian_timeout_ms: DEFAULT_HISTORIAN_TIMEOUT_MS,
                embedding: {
                    provider: "local",
                    model: DEFAULT_LOCAL_EMBEDDING_MODEL,
                },
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
            expect(result.historian).toBeUndefined();
            expect(result.dreamer).toBeUndefined();
            expect(result.sidekick).toBeUndefined();
            expect(result.pi).toBeUndefined();
            expect(result.mural).toEqual({ enabled: false });
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
                mural: { enabled: false },
                transform_mode: "ts",
                auto_update: false,
                toast_duration_ms: 5000,
                cache_ttl: "10m",
                prompt_surface: { default: "full" },
                protected_tags: 3,
                execute_threshold_percentage: 75,
                clear_reasoning_age: 60,
                history_budget_percentage: 0.2,
                historian_timeout_ms: 360_000,
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
                smart_notes: { retina_handoff: false },
                shadow_embedding: {
                    enabled: false,
                },
                caveman_text_compression: {
                    enabled: false,
                    min_chars: 500,
                },
                embedding: {
                    provider: "openai-compatible",
                    endpoint: "http://localhost:1234/v1",
                    model: "text-embedding-3-small",
                    api_key: "secret-embedding",
                },
                memory: {
                    enabled: true,
                    injection_budget_tokens: 4000,
                    auto_promote: true,
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
                sidekick: {
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

        it("applies sidekick defaults when the object is present", () => {
            const result = EidnaraConfigSchema.parse({
                sidekick: {
                    model: "github-copilot/gpt-5.4",
                },
            });

            expect(result.sidekick).toEqual({
                model: "github-copilot/gpt-5.4",
                timeout_ms: 30000,
            });
        });

        it("accepts disable on hidden agents and strips deprecated top-level enabled", () => {
            const result = EidnaraConfigSchema.parse({
                historian: { disable: true },
                dreamer: {
                    disable: true,
                    enabled: true,
                    // maintain-docs scheduled.
                    tasks: {
                        "review-user-memories": { schedule: "" },
                        "maintain-docs": { schedule: "0 * * * *" },
                    },
                },
                sidekick: { disable: true, enabled: true },
            });

            expect(result.historian?.disable).toBe(true);
            expect(result.dreamer?.disable).toBe(true);
            expect(result.sidekick?.disable).toBe(true);
            expect("enabled" in (result.dreamer as Record<string, unknown>)).toBe(false);
            expect("enabled" in (result.sidekick as Record<string, unknown>)).toBe(false);
            expect(result.dreamer?.tasks["review-user-memories"].schedule).toBe("");
            expect(result.dreamer?.tasks["maintain-docs"].schedule).toBe("0 * * * *");
            expect(result.dreamer?.tasks["classify-memories"].schedule).toBe("0 6 * * *");
            expect(result.dreamer?.tasks.retrospective.schedule).toBe("0 5 * * *");
        });

        it("defaults classify-memories and retrospective on daily in dreamer task schema", () => {
            const result = EidnaraConfigSchema.parse({ dreamer: { model: "x/y" } });
            expect(result.dreamer?.tasks["classify-memories"].schedule).toBe("0 6 * * *");
            expect(result.dreamer?.tasks.retrospective.schedule).toBe("0 5 * * *");
        });

        it("parses both transform modes", () => {
            expect(EidnaraConfigSchema.parse({ transform_mode: "ts" }).transform_mode).toBe("ts");
            expect(EidnaraConfigSchema.parse({ transform_mode: "rust" }).transform_mode).toBe(
                "rust",
            );
        });

        it("accepts optional auto_update user preference", () => {
            expect(EidnaraConfigSchema.parse({ auto_update: false }).auto_update).toBe(false);
            expect(EidnaraConfigSchema.parse({ auto_update: true }).auto_update).toBe(true);
        });

        it("accepts an explicitly configured Pi subagent extension allowlist", () => {
            expect(
                EidnaraConfigSchema.parse({
                    pi: { subagent_extensions: ["provider-package", "./local.ts"] },
                }).pi,
            ).toEqual({ subagent_extensions: ["provider-package", "./local.ts"] });
        });

        it("accepts and normalizes 2-letter ISO 639-1 language codes", () => {
            expect(EidnaraConfigSchema.parse({ language: "tr" }).language).toBe("tr");
            expect(EidnaraConfigSchema.parse({ language: "  ES " }).language).toBe("es");
            expect(EidnaraConfigSchema.parse({ language: "ja" }).language).toBe("ja");
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
                tool_descriptions: { ctx_search: "Search project context" },
            };

            expect(
                EidnaraConfigSchema.parse({ prompt_surface: promptSurface }).prompt_surface,
            ).toEqual(promptSurface);
        });
    });

    describe("validation", () => {
        it("rejects an unknown transform mode", () => {
            expect(() => EidnaraConfigSchema.parse({ transform_mode: "wasm" })).toThrow();
        });

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
                "*/model",
            ];
            for (const key of malformedKeys) {
                expect(
                    EidnaraConfigSchema.safeParse({
                        prompt_surface: { models: { [key]: "light" } },
                    }).success,
                ).toBe(false);
            }

            expect(
                EidnaraConfigSchema.safeParse({
                    prompt_surface: { guidance_override_path: "  " },
                }).success,
            ).toBe(false);
            expect(
                EidnaraConfigSchema.safeParse({
                    prompt_surface: { tool_descriptions: { ctx_search: "  " } },
                }).success,
            ).toBe(false);
            expect(
                EidnaraConfigSchema.safeParse({
                    prompt_surface: { tool_descriptions: { "  ": "description" } },
                }).success,
            ).toBe(false);
        });

        it("rejects empty Pi subagent extension entries", () => {
            expect(() =>
                EidnaraConfigSchema.parse({ pi: { subagent_extensions: ["  "] } }),
            ).toThrow();
        });

        it("rejects protected_tags greater than 100", () => {
            expect(() => EidnaraConfigSchema.parse({ protected_tags: 101 })).toThrow();
        });

        it("rejects protected_tags less than 1", () => {
            expect(() => EidnaraConfigSchema.parse({ protected_tags: 0 })).toThrow();
        });

        it("accepts protected_tags boundary values", () => {
            expect(EidnaraConfigSchema.parse({ protected_tags: 1 }).protected_tags).toBe(1);
            expect(EidnaraConfigSchema.parse({ protected_tags: 20 }).protected_tags).toBe(20);
        });

        it("rejects clear_reasoning_age below minimum", () => {
            expect(() => EidnaraConfigSchema.parse({ clear_reasoning_age: 9 })).toThrow();
        });

        it("rejects historian_timeout_ms below minimum", () => {
            expect(() => EidnaraConfigSchema.parse({ historian_timeout_ms: 59_999 })).toThrow();
        });

        it("rejects non-code output language values", () => {
            expect(() => EidnaraConfigSchema.parse({ language: "Turkish" })).toThrow(); // full name
            expect(() => EidnaraConfigSchema.parse({ language: "tur" })).toThrow(); // 3-letter
            expect(() => EidnaraConfigSchema.parse({ language: "zz" })).toThrow(); // unknown code
            expect(() => EidnaraConfigSchema.parse({ language: "<x>" })).toThrow();
        });

        it("rejects openai-compatible embedding config without endpoint", () => {
            expect(() =>
                EidnaraConfigSchema.parse({
                    embedding: {
                        provider: "openai-compatible",
                        model: "text-embedding-3-small",
                    },
                }),
            ).toThrow();
        });

        it("rejects openai-compatible embedding config without model", () => {
            expect(() =>
                EidnaraConfigSchema.parse({
                    embedding: {
                        provider: "openai-compatible",
                        endpoint: "http://localhost:1234/v1",
                    },
                }),
            ).toThrow();
        });

        it("accepts a configured local embedding dtype", () => {
            const result = EidnaraConfigSchema.parse({
                embedding: {
                    provider: "local",
                    model: "Xenova/paraphrase-multilingual-MiniLM-L12-v2",
                    local_dtype: "q8",
                },
            });
            expect(result.embedding).toEqual({
                provider: "local",
                model: "Xenova/paraphrase-multilingual-MiniLM-L12-v2",
                local_dtype: "q8",
            });
        });

        it("omits local_dtype from the resolved config when unset (preserves default identity)", () => {
            const result = EidnaraConfigSchema.parse({
                embedding: { provider: "local" },
            });
            expect(result.embedding).toEqual({
                provider: "local",
                model: DEFAULT_LOCAL_EMBEDDING_MODEL,
            });
            expect("local_dtype" in result.embedding).toBe(false);
        });

        it("rejects an unsupported local embedding dtype", () => {
            expect(() =>
                EidnaraConfigSchema.parse({
                    embedding: { provider: "local", local_dtype: "fp64" },
                }),
            ).toThrow();
        });
    });
});
