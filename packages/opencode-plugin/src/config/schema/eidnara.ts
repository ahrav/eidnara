import { homedir } from "node:os";
import { z } from "zod";
import { isValidLanguageCode } from "../../agents/language-directive";
import { DEFAULT_PROTECTED_TAGS } from "../../features/context/defaults";
import { PROMPT_SURFACE_MODEL_KEY_PATTERN } from "../../shared/prompt-surface";
import { AgentOverrideConfigSchema } from "./agent-overrides";

export const DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE = 65;
/** The project-config sanitizer shares these bounds so schema-valid project values cannot bypass its raise-only rule. */
export const MIN_EXECUTE_THRESHOLD_PERCENTAGE = 20;
export const MAX_EXECUTE_THRESHOLD_PERCENTAGE = 90;
export const MIN_EXECUTE_THRESHOLD_TOKENS = 5_000;
export const MAX_EXECUTE_THRESHOLD_TOKENS = 2_000_000;
// The 95% emergency wall remains above the 90% execute-threshold cap.
export const EXECUTE_THRESHOLD_CAP_MESSAGE = `execute_threshold is capped at ${MAX_EXECUTE_THRESHOLD_PERCENTAGE}% for cache safety: output capacity is reserved from the usable context window, and the remaining ${100 - MAX_EXECUTE_THRESHOLD_PERCENTAGE}% absorbs mid-turn growth before the absolute 95% emergency wall. Use a value between ${MIN_EXECUTE_THRESHOLD_PERCENTAGE} and ${MAX_EXECUTE_THRESHOLD_PERCENTAGE}.`;
export const DEFAULT_HISTORY_SUMMARIZER_TIMEOUT_MS = 300_000;
/** Upper bound a session may configure for `memory.injection_budget_tokens`. */
export const MAX_MEMORY_INJECTION_BUDGET_TOKENS = 20_000;
export const DEFAULT_HISTORY_BUDGET_PERCENTAGE = 0.15;
const LANGUAGE_CODE_MESSAGE = 'language must be a 2-letter ISO 639-1 code (e.g. "tr", "es", "de")';
// `.regex(/\S/)` publishes as a JSON Schema `pattern`. `.trim().min(1)` publishes only
// `minLength: 1`, and a `.refine` on `trim().length` publishes nothing, so both let editors
// accept whitespace-only values the loader rejects.
const NON_BLANK_PATTERN = /\S/;

/** The schema generator matches `LanguageCodeSchema` by identity and publishes the codes `isValidLanguageCode` accepts as a `pattern`. */
export const LanguageCodeSchema = z
    .string()
    .regex(/^\s*[A-Za-z]{2}\s*$/, { message: LANGUAGE_CODE_MESSAGE, abort: true })
    .trim()
    .toLowerCase()
    .refine((s) => isValidLanguageCode(s), LANGUAGE_CODE_MESSAGE);

/** PiThinkingLevelSchema maps to Pi's `--thinking` CLI flag.
 * `off` disables reasoning; `minimal` through `max` increase reasoning depth.
 *  Pi-only — OpenCode uses `variant` in agent config instead. */
export const PiThinkingLevelSchema = z
    .enum(["off", "minimal", "low", "medium", "high", "xhigh", "max"])
    .optional();
export type PiThinkingLevel = z.infer<typeof PiThinkingLevelSchema>;

/** An absent allowlist preserves Pi's normal extension discovery behavior.
 * */
export const PiConfigSchema = z
    .strictObject({
        subagent_extensions: z
            .array(z.string().trim().regex(NON_BLANK_PATTERN))
            .optional()
            .describe(
                "User-only allowlist of Pi extensions for Eidnara subagent children. When set, children use --no-extensions and load only these entries (plus Eidnara's scoped child extension where applicable). Relative paths resolve from ~/.pi/agent, matching Pi's settings.json package location. Unset preserves normal Pi extension discovery.",
            ),
    })
    .optional();
export type PiConfig = NonNullable<z.infer<typeof PiConfigSchema>>;

/**
 * PromptSurfacePresetSchema routes the built-in prompt surface without changing guidance or tool registration.
 * Project config can choose preset routing; only user config can provide override text.
 */
export const PromptSurfacePresetSchema = z.enum(["full", "light"]);
export type { PromptSurfacePreset } from "../../shared/prompt-surface";

const PromptSurfaceModelKeySchema = z.string().regex(PROMPT_SURFACE_MODEL_KEY_PATTERN, {
    message:
        "Use a non-empty bare model key, provider/model key, or the literal provider/* wildcard; model IDs may contain additional slashes and matching is case-sensitive.",
});
// Harness-specific known-tool validation runs when a user override is applied.
const PromptSurfaceToolKeySchema = z.string().regex(NON_BLANK_PATTERN, {
    message: "tool description keys must not be empty or whitespace-only",
});

export const PromptSurfaceConfigSchema = z
    .strictObject({
        default: PromptSurfacePresetSchema.default("full").describe(
            'Fallback prompt-surface preset ("full" or "light").',
        ),
        models: z
            .record(PromptSurfaceModelKeySchema, PromptSurfacePresetSchema)
            .optional()
            .describe(
                "Literal per-model routing. Keys are bare model IDs, provider/model, or provider/*; matching is case-sensitive and preserves additional slashes in model IDs.",
            ),
        guidance_override_path: z
            .string()
            .regex(NON_BLANK_PATTERN, {
                message: "guidance_override_path must not be empty or whitespace-only",
            })
            .optional()
            .describe(
                "USER-LEVEL ONLY path to a complete primary guidance section. Relative paths resolve from the user config file.",
            ),
        tool_descriptions: z
            .record(
                PromptSurfaceToolKeySchema,
                z.string().regex(NON_BLANK_PATTERN, {
                    message: "tool description values must not be empty or whitespace-only",
                }),
            )
            .optional()
            .describe(
                "USER-LEVEL ONLY top-level description overrides keyed by eidnara_* tool ID; parameter schemas and descriptions are unchanged.",
            ),
    })
    .describe(
        "Prompt-surface preset routing. Project config may select default/models, while guidance_override_path and tool_descriptions are user-level only.",
    );
export type PromptSurfaceConfig = z.infer<typeof PromptSurfaceConfigSchema>;

export const ContextResearcherConfigSchema = AgentOverrideConfigSchema.extend({
    timeout_ms: z
        .number()
        .default(30000)
        .describe("Timeout for context_researcher calls in milliseconds"),
    system_prompt: z.string().optional().describe("Custom system prompt for context_researcher"),
    thinking_level: PiThinkingLevelSchema.describe(
        "Pi only: explicit thinking level for context_researcher subagent invocations. See history_summarizer.thinking_level.",
    ),
}).optional();
export type ContextResearcherConfig = NonNullable<z.infer<typeof ContextResearcherConfigSchema>>;

export const HistorySummarizerConfigSchema = AgentOverrideConfigSchema.extend({
    thinking_level: PiThinkingLevelSchema.describe(
        "Pi only: explicit thinking level passed as --thinking <level> to Pi history_summarizer subagent invocations. Required when using reasoning models (e.g. github-copilot/gpt-5.4) because Pi's default thinking-level resolution can pick a value the provider rejects. OpenCode users set variant instead. Valid: off | minimal | low | medium | high | xhigh | max",
    ),
    disallowed_tools: z
        .array(z.enum(["*", "read", "aft_outline", "aft_zoom", "aft_search"]))
        .default([])
        .describe(
            'OpenCode only. Tools to REMOVE from the history_summarizer\'s default allow-list [read, aft_outline, aft_zoom, aft_search]. Use ["*"] to strip all tool definitions from the model request — this prevents weak instruction-following models (e.g. mistral-small-latest) from entering tool-calling loops. Individual tool names remove just that tool. Note: a user-supplied history_summarizer.permission override can re-allow a tool that disallowed_tools removed — disallowed_tools sets the baseline, permission overrides take precedence. (default: [])',
        ),
}).optional();
export type HistorySummarizerConfig = NonNullable<z.infer<typeof HistorySummarizerConfigSchema>>;

function expandConfigPath(value: string): string {
    const trimmed = value.trim();
    if (trimmed === "~") return homedir();
    if (trimmed.startsWith("~/")) return `${homedir()}/${trimmed.slice(2)}`;
    return trimmed;
}

export interface HostConnectionConfig {
    connection_file: string;
}

export interface ShadowEmbeddingConfig {
    enabled: boolean;
}

export interface EidnaraConfig {
    enabled: boolean;
    /** User-level setting that lets a session started exactly in the canonical home directory use a deterministic directory identity. */
    allow_home_project: boolean;
    /** Selects the runtime implementation for this project. Rust mode is experimental and requires user-level host configuration. */
    transform_mode: "ts" | "rust";
    /** Only user config can set the output language for generated Eidnara prose. */
    language?: string;
    history_summarizer?: HistorySummarizerConfig;
    cache_ttl: string | { default: string; [modelKey: string]: string };
    /** The preset routes guidance and provider-visible prompt surfaces. */
    prompt_surface: PromptSurfaceConfig;
    /** Only user config can override output-token reservation; 0 disables reservation. */
    output_reserve?: number | { default: number; [modelKey: string]: number };
    /** Only user config can provide model metadata. */
    models?: { window_overlay_path?: string };
    /* */
    toast_duration_ms?: number;
    execute_threshold_percentage: number | { default: number; [modelKey: string]: number };
    /**
     * `execute_threshold_tokens` overrides `execute_threshold_percentage` for models with a model-specific or default token threshold.
     * These thresholds support hard caps matching provider input limits; Eidnara clamps values above 90% × `context_limit` and warns. */
    execute_threshold_tokens?: { default?: number; [modelKey: string]: number | undefined };
    protected_tags: number;
    clear_reasoning_age: number;
    history_budget_percentage: number;
    history_summarizer_timeout_ms: number;
    commit_cluster_trigger: {
        enabled: boolean;
        min_clusters: number;
    };
    /** Eidnara applies these SQLite settings per connection to its own `context.db`. */
    sqlite: {
        cache_size_mb: number;
        mmap_size_mb: number;
    };
    /**
     * An external operator retains control of shared-storage permissions.
     * Only user config can set `storage.enforce_private_permissions`; project configs cannot weaken local data privacy.
     */
    storage: {
        enforce_private_permissions: boolean;
    };
    /**
     * Eidnara can inject `## Eidnara` guidance, `<project-docs>`, `<user-profile>`, and a sticky date.
     * Eidnara injects this content through `experimental.chat.system.transform`.
     *
     * Eidnara skips hidden OpenCode title, summary, and compaction agents through a separate code path.
     */
    system_prompt_injection: {
        /** When false, Eidnara injects no system-prompt content. */
        enabled: boolean;
        /**
         * Eidnara skips all system-prompt injection when an agent system prompt contains a configured signature.
         */
        skip_signatures: string[];
    };
    /** Eidnara injects elapsed-time markers between user messages and date ranges on history_segments.
     * Default: true. */
    temporal_awareness: boolean;
    /** When true, Eidnara retains child sessions spawned for history_summarizer, memory_classifier, context_researcher, and memory migration; retained sessions accumulate until manually cleared. Default: false.
     * */
    keep_subagents: boolean;
    /**
     * `fail_closed_blocking` blocks primary-session transforms when a schema fence or storage open/migration failure makes Eidnara inoperable.
     * A storage open or migration failure blocks the primary-session transform with a recovery error.
     * Deterministic inoperability raises a recovery error instead of falling through to native compaction.
     * Only user configuration can set `fail_closed_blocking`; the project tier cannot set it.
     */
    fail_closed_blocking: boolean;
    /**
     * When `compaction.enabled` is false, Eidnara stops managing the context window.
     * When `compaction.enabled` is false, Eidnara retains only its knowledge layer.
     * When `compaction.enabled` is false, the harness's native compaction, or no compaction, manages the context window.
     * Only user configuration can set `compaction.enabled`; Eidnara strips project-tier values.
     * `compaction.enabled` resolves at boot; changing it requires a process restart.
     */
    compaction: {
        enabled: boolean;
    };
    /** Eidnara exposes its OpenCode-parity `todowrite` surface only on Pi. */
    todowrite: {
        enabled: boolean;
        overlay: boolean;
    };
    /** Only Pi exposes child-process extension controls. */
    pi?: PiConfig;
    /** `smart_drops` reclaims tool output that later calls supersede in addition to normal age-based auto-drop.
     * `smart_drops` drops superseded `todowrite`, `eidnara_reduce`, and `meta` outputs.
     * `smart_drops` replaces older edits to the same file with a marker.
     * The replacement marker retains only `filePath`; `smart_drops` runs only during a message-rewriting transform pass.
     * Because `smart_drops` runs only during a message-rewriting transform pass, it never independently causes a prompt-cache miss.
     * When `smart_drops` is false, the messages sent to the model are byte-identical.
     * `smart_drops` is disabled by default.
     * */
    smart_drops: boolean;
    /**
     * `terse_text_compression` applies age-tier compression to long user and assistant text parts.
     * `terse_text_compression` is opt-in and disabled by default.
     *
     * `terse_text_compression` runs only for primary sessions and never for subagents.
     * `terse_text_compression` groups eligible messages outside the protected tail into four age tiers by tag position.
     * The oldest eligible 20% uses `ultra` compression, and the next 20% uses `full` compression.
     * The next eligible 20% uses `lite` compression; the newest 40% remains untouched.
     * `terse_text_compression` rewrites the eligible text part in place.
     * `terse_text_compression` always compresses from `source_contents`.
     * Tier shifts produce the same output as applying the target depth directly to the original text.
     *
     */
    terse_text_compression: {
        enabled: boolean;
        /** Text parts shorter than `min_chars` are left untouched. */
        min_chars: number;
    };
    /** `host` provides user-only connection settings for the LocalEmbeddings daemon. */
    host?: HostConnectionConfig;
    /** Only developers can enable `shadow_embedding`. */
    shadow_embedding?: ShadowEmbeddingConfig;
    memory: {
        enabled: boolean;
        injection_budget_tokens: number;
        auto_promote: boolean;
        retrieval_count_promotion_threshold: number;
        /** `auto_search` appends a compact hint to new user messages when `eidnara_search` finds related results.
         * `auto_search` injects fragments rather than full content.
         * Hints direct agents to run `eidnara_search` for full context when relevant.
         * `auto_search` is enabled by default and operates independently of `memory.enabled`.
         * `auto_search` can surface conversation and Git hints when `memory.enabled` is false.
         * */
        auto_search: {
            enabled: boolean;
            /** Top hit score must exceed this threshold for the hint to fire. */
            score_threshold: number;
            /** `auto_search` skips user messages shorter than `min_prompt_chars`. */
            min_prompt_chars: number;
        };
        /** `git_commit_indexing` indexes commit messages reachable from HEAD in a `eidnara_search` source.
         * `git_commit_indexing` lets agents recall recent regressions, fixes, and decisions from indexed commit messages.
         * `git_commit_indexing` is opt-in, defaults to off, and operates independently of `memory.enabled`.
         *  of `memory.enabled`. */
        git_commit_indexing: {
            enabled: boolean;
            /** `git_commit_indexing` indexes `since_days` days of history (default: 365). */
            since_days: number;
            /** `git_commit_indexing` retains at most `max_commits` commits per project, evicting the oldest (default: 2000). */
            max_commits: number;
        };
    };
    context_researcher?: ContextResearcherConfig;
}

export const EidnaraConfigSchema = z
    .strictObject({
        $schema: z.string().optional(),
        disabled_hooks: z.array(z.string()).optional(),
        command: z
            .record(
                z.string(),
                z.strictObject({
                    template: z.string(),
                    description: z.string().optional(),
                    agent: z.string().optional(),
                    model: z.string().optional(),
                    subtask: z.boolean().optional(),
                }),
            )
            .optional(),
        enabled: z.boolean().default(true).describe("Enable Eidnara (default: true)"),
        allow_home_project: z
            .boolean()
            .default(false)
            .describe(
                "Allow Eidnara sessions launched from the exact canonical home directory. The home session uses its deterministic dir: identity so pre-gate memories reconnect. USER-LEVEL ONLY: project config is ignored. The home identity is excluded from registry seed exports, never resolves descendants by containment, and cannot join a workspace.",
            ),
        transform_mode: z
            .enum(["ts", "rust"])
            .default("ts")
            .describe(
                'Experimental: routes the project through the direct Rust daemon (requires the user-level host.connection_file path); "ts" is the current TypeScript pipeline.',
            ),
        language: LanguageCodeSchema.optional().describe(
            "Output language for Eidnara's generated content and guidance, as a " +
                '2-letter ISO 639-1 code (e.g. "tr", "es", "de", "ja", "pt"). When set, the ' +
                "history_summarizer, memory_classifier, context_researcher, and the agent-guidance block instruct the model to " +
                "write its PROSE in this language while keeping all structural tokens (XML tags, " +
                "the five memory category names, code identifiers, file paths) in English. " +
                "USER-LEVEL ONLY (ignored in project config for security). Unset = today's " +
                "behavior (model mirrors the conversation; English scaffolding). Changing it " +
                "triggers one cache re-materialization; existing history_segments/memories keep their " +
                "original language until naturally rewritten.",
        ),
        history_summarizer: HistorySummarizerConfigSchema.describe(
            "HistorySummarizer agent configuration (model, fallback_models, variant, temperature, maxTokens, permission, etc.)",
        ),
        cache_ttl: z
            .union([z.string(), z.strictObject({ default: z.string() }).catchall(z.string())])
            .default("5m")
            .describe(
                'Cache TTL: string (e.g. "5m", "1h", "30s") or per-model object ({ default: "5m", "model-id": "10m" }). Only user configuration can set either shape; project values are ignored with a warning. Set to "never" for lanes kept warm by an external keepwarm proxy; this disables the idle-TTL heuristic so the plugin never initiates a rebuild based on elapsed time.',
            ),
        prompt_surface: PromptSurfaceConfigSchema.default({ default: "full" }).describe(
            "Prompt-surface presets: default is full; models use bare model IDs, provider/model, or provider/* routing keys. Guidance and tool-description overrides are user-level only. On OpenCode and Pi, per-model routing applies to the guidance block only: tool descriptions are registered once per process, so they follow the default preset (a v1 plugin-surface limitation; per-model tool descriptions are planned for the OpenCode v2 plugin API once the SDK stabilizes).",
        ),
        output_reserve: z
            .union([
                z.number().min(0),
                z.strictObject({ default: z.number().min(0) }).catchall(z.number().min(0)),
            ])
            .optional()
            .describe(
                'User-only output-token reservation override. Number or per-model object ({ default: 16384, "provider/model": 8192 }); 0 disables reservation. Takes precedence over every derived source: an explicit value here always wins against catalog output limits, provider window-geometry facts, and the 25%-of-context fallback (usable window = context window minus this reserve). When unset, Eidnara reserves the catalog output limit (capped at 25% of context) for shared-window providers and keeps proven separate-quota Google/Gemini windows unchanged.',
            ),
        models: z
            .strictObject({
                window_overlay_path: z.string().trim().regex(NON_BLANK_PATTERN).optional(),
            })
            .optional()
            .describe(
                "User-only Fusiform window-overlay settings. The path defaults to <dataDir>/fusiform/window-overlay.json.",
            ),
        toast_duration_ms: z
            .number()
            .min(0)
            .max(60_000)
            .default(5_000)
            .describe(
                "TUI toast lifetime in milliseconds for Eidnara notifications. Set to 0 to disable Eidnara toasts entirely (min: 0, max: 60000, default: 5000)",
            ),
        execute_threshold_percentage: z
            .union([
                z
                    .number()
                    .min(MIN_EXECUTE_THRESHOLD_PERCENTAGE)
                    .max(MAX_EXECUTE_THRESHOLD_PERCENTAGE, EXECUTE_THRESHOLD_CAP_MESSAGE),
                z
                    .strictObject({
                        // Optional so a per-model map without `default` validates; the transform
                        // fills `DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE`.
                        default: z
                            .number()
                            .min(MIN_EXECUTE_THRESHOLD_PERCENTAGE)
                            .max(MAX_EXECUTE_THRESHOLD_PERCENTAGE, EXECUTE_THRESHOLD_CAP_MESSAGE)
                            .optional(),
                    })
                    .catchall(
                        z
                            .number()
                            .min(MIN_EXECUTE_THRESHOLD_PERCENTAGE)
                            .max(MAX_EXECUTE_THRESHOLD_PERCENTAGE, EXECUTE_THRESHOLD_CAP_MESSAGE),
                    ),
            ])
            .default(DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE)
            .describe(
                'Context percentage that forces queued operations to execute. Number or per-model object ({ default: 65, "provider/model": 45 }); `default` falls back to DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE when omitted. Values above 90 are rejected because the runtime caps at 90% of the output-reserved safe window (MAX_EXECUTE_THRESHOLD). Default: DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE',
            ),
        execute_threshold_tokens: z
            .strictObject({
                default: z
                    .number()
                    .min(MIN_EXECUTE_THRESHOLD_TOKENS)
                    .max(MAX_EXECUTE_THRESHOLD_TOKENS)
                    .optional(),
            })
            .catchall(
                z.number().min(MIN_EXECUTE_THRESHOLD_TOKENS).max(MAX_EXECUTE_THRESHOLD_TOKENS),
            )
            .optional()
            .describe(
                "Absolute token thresholds per model. When matched, overrides execute_threshold_percentage for that model. Accepts `default` for all models or per-model keys. Values above 90% × context_limit are clamped with a warning log. Min 5_000, max 2_000_000.",
            ),
        protected_tags: z
            .number()
            .min(1)
            .max(100)
            .optional()
            .describe(
                "Number of recent tags to protect from dropping (min: 1, max: 100, default: 20)",
            ),
        clear_reasoning_age: z
            .number()
            .min(10)
            .default(50)
            .describe("Clear reasoning/thinking blocks older than N tags (default: 50)"),
        history_budget_percentage: z
            .number()
            .min(0.05)
            .max(0.5)
            .default(DEFAULT_HISTORY_BUDGET_PERCENTAGE)
            .describe(
                "Fraction of usable context (context_limit × execute_threshold) reserved for the session history block (default: 0.15)",
            ),
        history_summarizer_timeout_ms: z
            .number()
            .min(60_000)
            .default(DEFAULT_HISTORY_SUMMARIZER_TIMEOUT_MS)
            .describe(
                "Timeout for each history_summarizer prompt call in milliseconds (default: 300000)",
            ),
        commit_cluster_trigger: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(true)
                    .describe(
                        "Enable commit-cluster based history_summarizer triggering (default: true)",
                    ),
                min_clusters: z
                    .number()
                    .min(1)
                    .default(3)
                    .describe(
                        "Minimum commit clusters required to trigger history_summarizer (min: 1, default: 3)",
                    ),
            })
            .default({ enabled: true, min_clusters: 3 })
            .describe(
                "Commit-cluster trigger: fire history_summarizer when enough commit clusters accumulate in the unsummarized tail",
            ),
        system_prompt_injection: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(true)
                    .describe(
                        "When false, NO injection happens for ANY agent — global escape hatch. (default: true)",
                    ),
                skip_signatures: z
                    .array(z.string())
                    .default(["<!-- eidnara: skip -->"])
                    .describe(
                        "Substring opt-out list. If the agent's system prompt contains any of these strings, skip ALL Eidnara injection for that call. Default \"<!-- eidnara: skip -->\" is meant to be added inside a user's custom agent prompt to opt that agent out.",
                    ),
            })
            .default({
                enabled: true,
                skip_signatures: ["<!-- eidnara: skip -->"],
            })
            .describe(
                "Controls whether and where Eidnara augments the system prompt. Lets users opt specific agents out of the Eidnara guidance and the surrounding project-docs / user-profile blocks. OpenCode's internal hidden agents — title, summary, and compaction — are always skipped automatically.",
            ),
        sqlite: z
            .strictObject({
                cache_size_mb: z
                    .number()
                    .min(2)
                    .max(2048)
                    .default(64)
                    .describe(
                        "Page-cache size in MiB per connection (PRAGMA cache_size). Larger keeps more hot pages resident, cutting re-reads on repeated full-table scans. (min 2, max 2048, default 64)",
                    ),
                mmap_size_mb: z
                    .number()
                    .min(0)
                    .max(8192)
                    .default(0)
                    .describe(
                        "Memory-mapped I/O size in MiB (PRAGMA mmap_size). 0 disables mmap (SQLite default). Raising it can cut read overhead on large DBs at the cost of address space. (min 0, max 8192, default 0)",
                    ),
            })
            .default({ cache_size_mb: 64, mmap_size_mb: 0 })
            .describe(
                "SQLite connection tuning for Eidnara's own context.db. These are per-connection PRAGMAs applied at open; they do not change the schema or what is stored.",
            ),
        storage: z
            .strictObject({
                enforce_private_permissions: z
                    .boolean()
                    .default(true)
                    .describe(
                        "When true (default), Eidnara creates and re-tightens its storage directories to owner-only 0700 and storage files to owner-only 0600. Set false only for a deliberate trusted-group deployment whose operator manages directory, database, WAL/SHM, cache, and RPC file permissions externally; Eidnara then never chmods or supplies restrictive creation modes. USER-LEVEL ONLY — ignored in project config for security. On Windows, POSIX chmod modes are already meaningless, so this setting is a no-op.",
                    ),
            })
            .default({ enforce_private_permissions: true })
            .describe(
                "Storage permission policy. The default keeps session content and memories owner-private. Disabling enforcement is for trusted shared-group storage managed externally; every group member able to read the storage can read all stored session content and memories.",
            ),
        host: z
            .strictObject({
                connection_file: z
                    .string()
                    .trim()
                    .regex(NON_BLANK_PATTERN)
                    .transform(expandConfigPath)
                    .describe("Path to the owner-only host connection file."),
            })
            .optional()
            .describe("User-only LocalEmbeddings daemon connection settings."),
        shadow_embedding: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(false)
                    .describe("Developer-only LocalEmbeddings shadow embedding lane switch."),
            })
            .default({ enabled: false })
            .describe("Developer-only LocalEmbeddings shadow embedding lane."),
        temporal_awareness: z
            .boolean()
            .default(true)
            .describe(
                'Inject wall-clock gap markers (<!-- +Xm -->) between user messages where > 5 min elapsed since the previous message, and add compact date ranges to history_segment headings. Gives the agent a sense of session pacing and "how long ago" across multi-day sessions. Graduated from experimental.temporal_awareness; default: true (set false to opt out).',
            ),
        keep_subagents: z
            .boolean()
            .default(false)
            .describe(
                "Debug: keep the child sessions Eidnara spawns for its own subagents (history_summarizer, memory_classifier, context_researcher, memory-migration) instead of deleting them on success. Useful for short-term inspection/data collection — their full transcript (prompt, tool calls, token usage, output) stays in the host session store. Kept sessions accumulate until manually cleared; leave false for normal use. Requires a restart to take effect.",
            ),
        fail_closed_blocking: z
            .boolean()
            .default(true)
            .describe(
                "When Eidnara cannot operate (schema fence mismatch, storage open/migration failure), block the primary-session prompt with a loud recovery error instead of silently degrading to native compaction. Default true. Set false only to restore the old degrade-silently behavior (not recommended). USER-LEVEL ONLY — ignored in project config for security. Requires a restart.",
            ),
        compaction: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(true)
                    .describe(
                        "When false, Eidnara stops managing the context window and keeps its knowledge layer: memory and docs/user-profile/key-files injection through additive m[0]/m[1], raw-message FTS indexing, memory_classifier, notes, eidnara_search, and eidnara_memory remain available. Eidnara's history_summarizer/history_segment preparation, tagging, markers, pruning, folding, drops, strips, splicing, synthetic context-management todos, temporal markers, nudges, and fail-closed blocking stop. fail_closed_blocking is inert: a transform failure passes the input messages through without blocking or cancelling. This setting does not enable native compaction: OpenCode's compaction.auto / compaction.prune or Pi's equivalent owns the window, or nothing does. Eidnara's compaction.enabled in eidnara.jsonc is distinct from OpenCode's compaction.auto / compaction.prune in opencode.jsonc; they are different files and different owners. On the first turn after disabling, a long session may trigger one native compaction cycle; Eidnara removes only its own marker boundary, leaves native boundaries and stored history_segments intact, and does no pre-trimming mitigation. Marker cleanup is lazy per session, so an unresumed session is cleaned when it is next resumed. If compaction is enabled again, run /eidnara-wrapup when the history_summarizer is runnable to catch up. OpenCode peer verification against v1.18.4 confirms native compaction covers child sessions: subagents receive additive memory/docs injection and no Eidnara reclaim in this mode, so keep subagent tasks small or leave compaction.enabled on for long subagent runs. This is boot-resolved and requires a process restart; project-tier compaction.enabled is stripped so a cloned repository cannot disable the user's setting. The sidebar reports raw usage as Context: <pct>% · native compaction or Context: <pct>% · no active compaction and does not show an Eidnara execute-threshold fill. /eidnara-wrapup, /eidnara-recomp, and /eidnara-flush refuse without context-management side effects. Raw content hidden by a native boundary before Eidnara's first pass is not retroactively indexed.",
                    ),
            })
            .default({ enabled: true })
            .describe(
                "Compaction-off mode gate. Default true (Eidnara manages the context window as today). Set compaction.enabled=false to keep the knowledge layer while letting native compaction (or nothing) own the window. Boot-resolved; requires a restart to change.",
            ),
        todowrite: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(true)
                    .describe(
                        "Pi only: register Eidnara's todowrite task-list tool. Disable if you use your own todo extension. OpenCode ships its own built-in todowrite; this setting has no effect there.",
                    ),
                overlay: z
                    .boolean()
                    .default(true)
                    .describe(
                        "Pi only: show the persistent todo overlay above the editor while tasks are active.",
                    ),
            })
            .default({ enabled: true, overlay: true })
            .describe(
                "Pi-only todowrite tool and overlay controls. Pi registers tools and widgets at extension boot, so changing this after /cd requires /reload or restart.",
            ),
        pi: PiConfigSchema.describe(
            "Pi-only child-process extension controls. This setting is user-level only; project configuration cannot choose which extensions a user's subagent children load.",
        ),
        smart_drops: z
            .boolean()
            .default(false)
            .describe(
                "Content-aware reclaim of provably-superseded tool output, layered on the existing execute-pass auto-drop. When on: superseded todowrite (keep newest 1), spent eidnara_reduce (keep newest 3), and zero-value meta (bash_status, bash_kill, eidnara_note read/dismiss) outputs are dropped; older edits to a file are compressed to a filePath-preserving marker while the newest edit per file stays full. Only acts on passes already busting the cache, so it never originates a cache bust. Honors the protected-tag reserve. Experimental: opt-in, default off until cache stability is proven; when off the wire is byte-identical to the positional-only reclaim. Requires a restart.",
            ),
        terse_text_compression: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(false)
                    .describe(
                        "Apply deterministic terse_text_compression-style text compression to old conversation text. Active for primary sessions when enabled; never for subagents. Compresses user/assistant text in oldest-first tiers: ultra (oldest 20%), full, lite, untouched (newest 40%).",
                    ),
                min_chars: z
                    .number()
                    .min(100)
                    .max(10000)
                    .default(500)
                    .describe(
                        "Text parts shorter than this (characters) stay untouched. Min 100, max 10000. Default: 500.",
                    ),
            })
            .default({ enabled: false, min_chars: 500 })
            .describe(
                "Age-tier terse_text_compression compression for long user/assistant text parts. Active for primary sessions when enabled; never for subagents. Oldest 20% of eligible tags (outside protected tail) go to ultra, next 20% to full, next 20% to lite, newest 40% untouched. Graduated from experimental.terse_text_compression; opt-in, default off (lossy).",
            ),
        memory: z
            .strictObject({
                enabled: z
                    .boolean()
                    .default(true)
                    .describe("Enable cross-session memory (default: true)"),
                injection_budget_tokens: z
                    .number()
                    .min(500)
                    .max(MAX_MEMORY_INJECTION_BUDGET_TOKENS)
                    .default(4000)
                    .describe(
                        "Token budget for memory injection on session start (min: 500, max: 20000, default: 4000). Only user configuration can set this budget; project values are ignored with a warning.",
                    ),
                auto_promote: z
                    .boolean()
                    .default(true)
                    .describe(
                        "Automatically promote eligible session facts into memory (default: true)",
                    ),
                retrieval_count_promotion_threshold: z
                    .number()
                    .min(1)
                    .default(3)
                    .describe(
                        "retrieval_count threshold for promoting memory to permanent status (min: 1, default: 3)",
                    ),
                auto_search: z
                    .strictObject({
                        enabled: z
                            .boolean()
                            .default(true)
                            .describe(
                                "Automatically append a compact <eidnara-search-hint> to eligible user messages when relevant memories, conversation, or commits are found. Graduated from experimental.auto_search; on by default (set false to opt out). Independent of memory.enabled.",
                            ),
                        score_threshold: z
                            .number()
                            .min(0.3)
                            .max(0.95)
                            .default(0.6)
                            .describe(
                                "Top hit score must exceed this threshold for the hint to fire (min: 0.3, max: 0.95, default: 0.60)",
                            ),
                        min_prompt_chars: z
                            .number()
                            .min(5)
                            .max(500)
                            .default(20)
                            .describe(
                                "Skip hint when user message is shorter than this (min: 5, max: 500, default: 20)",
                            ),
                    })
                    .default({ enabled: true, score_threshold: 0.6, min_prompt_chars: 20 })
                    .describe(
                        "Auto-search hint: transform-time eidnara_search on each new user message; when the top hit clears the threshold, append a compact <eidnara-search-hint> block of vague fragments to that user message. Does NOT inject full content. Graduated from experimental.auto_search; enabled by default (set enabled: false to opt out). Independent of memory.enabled.",
                    ),
                git_commit_indexing: z
                    .strictObject({
                        enabled: z
                            .boolean()
                            .default(false)
                            .describe(
                                "Index HEAD git commits for eidnara_search (git_commit source). Graduated from experimental.git_commit_indexing; opt-in, default off. Independent of memory.enabled.",
                            ),
                        since_days: z
                            .number()
                            .min(7)
                            .max(3650)
                            .default(365)
                            .describe(
                                "Days of HEAD history to index (min: 7, max: 3650, default: 365)",
                            ),
                        max_commits: z
                            .number()
                            .min(100)
                            .max(20000)
                            .default(2000)
                            .describe(
                                "Max commits kept per project; oldest evicted (min: 100, max: 20000, default: 2000)",
                            ),
                    })
                    .default({ enabled: false, since_days: 365, max_commits: 2000 })
                    .describe(
                        "Index git commit messages from HEAD into eidnara_search. Commits become a 4th searchable source alongside memories and session history. Graduated from experimental.git_commit_indexing; opt-in, default off (per-project embedding cost). Independent of memory.enabled.",
                    ),
            })
            .default({
                enabled: true,
                injection_budget_tokens: 4000,
                auto_promote: true,
                retrieval_count_promotion_threshold: 3,
                auto_search: { enabled: true, score_threshold: 0.6, min_prompt_chars: 20 },
                git_commit_indexing: { enabled: false, since_days: 365, max_commits: 2000 },
            })
            .describe("Cross-session memory configuration"),
        context_researcher: ContextResearcherConfigSchema.describe(
            "Optional context_researcher agent configuration for session-start memory retrieval",
        ),
    })
    .transform((data): EidnaraConfig => {
        const percentage = data.execute_threshold_percentage;
        return {
            ...data,
            execute_threshold_percentage:
                typeof percentage === "number"
                    ? percentage
                    : {
                          ...percentage,
                          default: percentage.default ?? DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE,
                      },
            protected_tags: data.protected_tags ?? DEFAULT_PROTECTED_TAGS,
        };
    });

/** Unknown owned keys fail before project filtering or tool registration; invalid known values retain their existing recovery policy. */
export function assertKnownConfigKeys(raw: Record<string, unknown>): void {
    const result = EidnaraConfigSchema.safeParse(raw);
    if (
        !result.success &&
        result.error.issues.some((issue) => issue.code === "unrecognized_keys")
    ) {
        throw new Error("Unknown Eidnara configuration key; update configuration before startup.");
    }
}
