import { resolveModelConfigOrDefault } from "../shared/prompt-surface";
import {
    DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE,
    MAX_EXECUTE_THRESHOLD_PERCENTAGE,
    MAX_EXECUTE_THRESHOLD_TOKENS,
    MIN_EXECUTE_THRESHOLD_PERCENTAGE,
    MIN_EXECUTE_THRESHOLD_TOKENS,
} from "./schema/eidnara";

/**
 *
 * A project config belongs to the cloned repository and is untrusted.
 * A repository config must never escalate privilege or exfiltrate secrets.
 * The project-config sanitizers run before and after repository config is merged over trusted user config.
 * The project-config sanitizers mutate raw project config in place and return human-readable warnings.
 * human-readable warnings.
 *
 */

/** These hidden agents run with elevated or autonomous capability. */
const HIDDEN_AGENT_KEYS = ["historian", "sidekick"] as const;
const HISTORIAN_USER_ONLY_FIELDS = ["model", "fallback_models", "disallowed_tools"] as const;
const PROMPT_SURFACE_USER_ONLY_FIELDS = ["guidance_override_path", "tool_descriptions"] as const;
/**
 * Every block below has at least one leaf that only user config may set. The leaf sanitizers
 * skip non-object blocks, so a project `null`, string, or array here would survive to the merge
 * and replace the trusted block wholesale.
 */
const USER_ONLY_LEAF_PARENTS = [
    "compaction",
    "models",
    "storage",
    "prompt_surface",
    "pi",
    "historian",
    "sidekick",
    "mural",
    "experimental",
] as const;

/**
 * An untrusted repository must not set these hidden-agent fields because they can escalate privileges or execute code.
 *
 * A repository-supplied `prompt` can reprogram a hidden agent, enabling unattended exfiltration or code execution.
 *  - `permission` — broadens the agent's per-tool permissions.
 * `tools` can enable a denied tool such as `bash` for an agent whose allow-list excludes it.
 * `system_prompt` takes precedence over Sidekick's built-in prompt, so a repository could reprogram Sidekick through `/ctx-aug` unless it is stripped.
 *                   via `/ctx-aug`.
 *
 * Historian model selection is user-only, and project compaction thresholds can only increase, preventing cloned repositories from forcing earlier compaction or extra Historian spending.
 */
const AGENT_ESCALATION_FIELDS = ["prompt", "permission", "tools", "system_prompt"] as const;
const PERCENTAGE_THRESHOLD_REASON =
    "security: a repository may only raise compaction thresholds above the user's effective value; it cannot force earlier historian work or cloned-repo cost escalation.";
const TOKEN_THRESHOLD_REASON =
    "security: a repository may only raise execute_threshold_tokens above the user's trusted token threshold; it cannot force earlier historian work or cloned-repo cost escalation.";
const TOKEN_THRESHOLD_INTRODUCTION_REASON =
    "security: a repository cannot introduce a new execute_threshold_tokens override when the user has no trusted token threshold for that key; that could force earlier historian work or cloned-repo cost escalation.";
const INVALID_THRESHOLD_REASON =
    "security: the value is not a valid threshold, so the user's trusted value is kept; an invalid project value must not fall back to the schema default below the user's setting.";

interface PercentageThresholdConfig {
    defaultValue: number;
    overrides: Map<string, number>;
}

interface TokenThresholdConfig {
    defaultValue: number | undefined;
    overrides: Map<string, number>;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isValidPercentageThreshold(value: unknown): value is number {
    return (
        typeof value === "number" &&
        Number.isFinite(value) &&
        value >= MIN_EXECUTE_THRESHOLD_PERCENTAGE &&
        value <= MAX_EXECUTE_THRESHOLD_PERCENTAGE
    );
}

function isValidTokenThreshold(value: unknown): value is number {
    return (
        typeof value === "number" &&
        Number.isFinite(value) &&
        value >= MIN_EXECUTE_THRESHOLD_TOKENS &&
        value <= MAX_EXECUTE_THRESHOLD_TOKENS
    );
}

function normalizeTrustedPercentageThresholds(value: unknown): PercentageThresholdConfig {
    if (typeof value === "number" && Number.isFinite(value)) {
        return { defaultValue: value, overrides: new Map() };
    }

    if (
        isPlainObject(value) &&
        typeof value.default === "number" &&
        Number.isFinite(value.default)
    ) {
        const overrides = new Map<string, number>();
        for (const [key, child] of Object.entries(value)) {
            if (key === "default") continue;
            if (typeof child === "number" && Number.isFinite(child)) {
                overrides.set(key, child);
            }
        }
        return { defaultValue: value.default, overrides };
    }

    return { defaultValue: DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE, overrides: new Map() };
}

function normalizeTrustedTokenThresholds(value: unknown): TokenThresholdConfig {
    if (!isPlainObject(value)) {
        return { defaultValue: undefined, overrides: new Map() };
    }

    const overrides = new Map<string, number>();
    for (const [key, child] of Object.entries(value)) {
        if (key === "default") continue;
        if (typeof child === "number" && Number.isFinite(child)) {
            overrides.set(key, child);
        }
    }

    return {
        defaultValue:
            typeof value.default === "number" && Number.isFinite(value.default)
                ? value.default
                : undefined,
        overrides,
    };
}

function clonePercentageThresholds(value: PercentageThresholdConfig): PercentageThresholdConfig {
    return {
        defaultValue: value.defaultValue,
        overrides: new Map(value.overrides),
    };
}

function cloneTokenThresholds(value: TokenThresholdConfig): TokenThresholdConfig {
    return {
        defaultValue: value.defaultValue,
        overrides: new Map(value.overrides),
    };
}

function percentageThresholdsEqual(
    left: PercentageThresholdConfig,
    right: PercentageThresholdConfig,
): boolean {
    if (left.defaultValue !== right.defaultValue) return false;
    if (left.overrides.size !== right.overrides.size) return false;
    for (const [key, value] of left.overrides) {
        if (right.overrides.get(key) !== value) return false;
    }
    return true;
}

function setMergedPercentageThreshold(
    mergedRaw: Record<string, unknown>,
    value: PercentageThresholdConfig,
): void {
    if (value.overrides.size === 0) {
        mergedRaw.execute_threshold_percentage = value.defaultValue;
        return;
    }

    const serialized: Record<string, number> = { default: value.defaultValue };
    for (const [key, threshold] of value.overrides) {
        serialized[key] = threshold;
    }
    mergedRaw.execute_threshold_percentage = serialized;
}

function setMergedTokenThreshold(
    mergedRaw: Record<string, unknown>,
    value: TokenThresholdConfig,
): void {
    if (value.defaultValue === undefined && value.overrides.size === 0) {
        delete mergedRaw.execute_threshold_tokens;
        return;
    }

    const serialized: Record<string, number> = {};
    if (value.defaultValue !== undefined) {
        serialized.default = value.defaultValue;
    }
    for (const [key, threshold] of value.overrides) {
        serialized[key] = threshold;
    }
    mergedRaw.execute_threshold_tokens = serialized;
}

function makeProjectThresholdWarning(field: string, reason: string): string {
    return `Ignoring ${field} from project config (${reason})`;
}

/** Model IDs sharing a non-dash prefix (`gpt-4` and `gpt-40`) do not shadow each other. */
function isDashPrefix(candidate: string, key: string): boolean {
    return (
        key.startsWith(candidate) &&
        (key.length === candidate.length || key[candidate.length] === "-")
    );
}

/**
 * Returns the trusted threshold that a project per-model key would shadow at runtime.
 * Qualified keys use `modelKeyLookupOrder`; bare keys can be reached from any provider.
 * A bare key must therefore exceed every matching trusted wildcard and dash-prefix.
 */
function resolveTrustedThreshold<T extends number | undefined>(
    base: { defaultValue: T; overrides: Map<string, number> },
    projectKey: string,
): T | number {
    const exact = base.overrides.get(projectKey);
    if (exact !== undefined) return exact;

    if (projectKey.includes("/")) {
        return resolveModelConfigOrDefault<T | number>(
            Object.fromEntries(base.overrides),
            projectKey,
            base.defaultValue,
        );
    }

    let effective: T | number = base.defaultValue;
    for (const [key, value] of base.overrides) {
        const slash = key.indexOf("/");
        const modelPart = slash >= 0 ? key.slice(slash + 1) : key;
        if (modelPart !== "*" && !isDashPrefix(modelPart, projectKey)) continue;
        if (effective === undefined || value > effective) effective = value;
    }
    return effective;
}

/**
 *
 * Closes:
 * A repository must not change `fail_closed_blocking`, which can unblock or force-block the loud inoperability gate.
 * Only user config may set `fail_closed_blocking` to `false`.
 * `allow_home_project` may establish a durable project identity only from user config.
 * Only user config may change `output_reserve` or `models.window_overlay_path`.
 *  - `language`: a repo must not inject prompt text through a user preference.
 * Only user config may set `sqlite` because its settings apply as PRAGMAs on the shared DB handle.
 * `sqlite` settings affect the shared DB handle used by every project in the process.
 * Project `sqlite` values could exhaust host memory or address space because they affect the shared DB handle.
 * Only user config may set `storage.enforce_private_permissions` because it changes the shared store's confidentiality.
 * Changing `storage.enforce_private_permissions` affects every session's local-memory confidentiality.
 * Only user config may enable an externally managed trusted-group deployment.
 * `transform_mode` may come from project config, but Rust activation also requires user-tier consent.
 * A project `transform_mode` selection can opt that project's runtime into the Rust pipeline.
 * Rust activation requires user-level `transform_mode` or trusted user-level `subc` configuration.
 * Rust can demand-start the managed native-host lifecycle only after user-tier consent.
 * Only user config may set `historian.model` or `historian.fallback_models` to prevent repositories from forcing compaction cost.
 * Only user config may set `historian.disallowed_tools`: the project tier merges over the user tier, so a project array would replace the user's removals and restore the historian's default tools.
 * Only user config may set `mural.model` so repositories cannot select a provider for project memory.
 * Project config must not set `pi.subagent_extensions` because it controls extensions loaded by Pi child processes.
 * A repository may select a reviewed `prompt_surface` preset but may not set arbitrary prompt text.
 * A repository may select a reviewed `prompt_surface` preset but may not inject arbitrary guidance or tool-description text.
 * Project config must not set hidden-agent `prompt`, `permission`, or `tools`.
 * Only user config may set hidden-agent `disable`: the project tier replaces the trusted leaf, so a project `disable: false` would reactivate an agent the user turned off, and disabling the historian would bypass the user-only `compaction.enabled` rule.
 * A project may not replace a block that carries user-only leaves with a non-object value: the merge would substitute the whole block for the trusted one, schema recovery would drop the invalid value, and the user's settings would fall back to defaults without any leaf ever being stripped.
 */
export function stripUnsafeProjectConfigFields(projectRaw: Record<string, unknown>): string[] {
    const warnings: string[] = [];

    for (const key of USER_ONLY_LEAF_PARENTS) {
        if (key in projectRaw && !isPlainObject(projectRaw[key])) {
            delete projectRaw[key];
            warnings.push(
                `Ignoring ${key} from project config (security: a repository cannot replace a block that carries user-only settings; a non-object value would discard the user's ${key} configuration).`,
            );
        }
    }

    if ("fail_closed_blocking" in projectRaw) {
        delete projectRaw.fail_closed_blocking;
        warnings.push(
            "Ignoring fail_closed_blocking from project config (security: only user-level config may disable or force the loud inoperability gate).",
        );
    }

    if ("allow_home_project" in projectRaw) {
        delete projectRaw.allow_home_project;
        warnings.push(
            "Ignoring allow_home_project from project config (security: only user-level config may opt the user's home directory into Eidnara).",
        );
    }

    const compaction = projectRaw.compaction;
    if (isPlainObject(compaction) && "enabled" in compaction) {
        delete compaction.enabled;
        warnings.push(
            "Ignoring compaction.enabled from project config (security: only user-level config may disable Eidnara's context-window management; a cloned repo cannot change how the user's window is owned).",
        );
    }

    if ("output_reserve" in projectRaw) {
        delete projectRaw.output_reserve;
        warnings.push(
            "Ignoring output_reserve from project config (security: output-token reservation only honors user-level config).",
        );
    }

    const models = projectRaw.models;
    if (isPlainObject(models) && "window_overlay_path" in models) {
        delete models.window_overlay_path;
        warnings.push(
            "Ignoring models.window_overlay_path from project config (security: only user-level config may select model geometry metadata).",
        );
    }

    if ("language" in projectRaw) {
        delete projectRaw.language;
        warnings.push(
            "Ignoring language from project config (security: output language is a user-level setting).",
        );
    }

    if ("sqlite" in projectRaw) {
        delete projectRaw.sqlite;
        warnings.push(
            "Ignoring sqlite.* from project config (security: SQLite cache/mmap PRAGMAs apply to the " +
                "process-global shared database handle; only user-level config may set them).",
        );
    }

    const storage = projectRaw.storage;
    if (isPlainObject(storage) && "enforce_private_permissions" in storage) {
        delete storage.enforce_private_permissions;
        warnings.push(
            "Ignoring storage.enforce_private_permissions from project config (security: only user-level config may opt into externally managed shared storage permissions).",
        );
    }

    const promptSurface = projectRaw.prompt_surface;
    if (isPlainObject(promptSurface)) {
        const removed: string[] = [];
        for (const field of PROMPT_SURFACE_USER_ONLY_FIELDS) {
            if (field in promptSurface) {
                delete promptSurface[field];
                removed.push(field);
            }
        }
        if (removed.length > 0) {
            warnings.push(
                `Ignoring prompt_surface.${removed.join("/")} from project config (security: repositories may select prompt presets but only user config may provide guidance or tool-description text).`,
            );
        }
    }

    const pi = projectRaw.pi;
    if (isPlainObject(pi) && "subagent_extensions" in pi) {
        delete pi.subagent_extensions;
        warnings.push(
            "Ignoring pi.subagent_extensions from project config (security: only user-level config may choose extensions loaded by Pi subagent children).",
        );
    }

    for (const field of ["subc", "shadow_embedding"] as const) {
        if (field in projectRaw) {
            delete projectRaw[field];
            warnings.push(
                `Ignoring ${field} from project config (security: daemon routing and developer-only embedding traffic are user-level settings).`,
            );
        }
    }

    const historian = projectRaw.historian;
    if (isPlainObject(historian)) {
        const removed: string[] = [];
        for (const field of HISTORIAN_USER_ONLY_FIELDS) {
            if (field in historian) {
                delete historian[field];
                removed.push(field);
            }
        }
        if (removed.length > 0) {
            warnings.push(
                `Ignoring historian.${removed.join("/")} from project config ` +
                    "(security: historian model selection and tool restrictions are user-level only; a repository cannot force extra compaction cost or re-enable a tool the user removed).",
            );
        }
    }

    const mural = projectRaw.mural;
    if (isPlainObject(mural) && "model" in mural) {
        delete mural.model;
        warnings.push(
            "Ignoring mural.model from project config (security: the mural cue-compressor model is a user-level setting; a repository cannot choose where project memory is sent).",
        );
    }

    const experimental = projectRaw.experimental;
    if (isPlainObject(experimental) && "mural" in experimental) {
        if (!isPlainObject(experimental.mural)) {
            delete experimental.mural;
            warnings.push(
                "Ignoring experimental.mural from project config (security: a repository cannot replace a block that carries user-only settings; a non-object value would discard the user's legacy mural configuration).",
            );
        } else if ("model" in experimental.mural) {
            delete experimental.mural.model;
            warnings.push(
                "Ignoring experimental.mural.model from project config (security: the mural cue-compressor model is a user-level setting; use user-level mural.model).",
            );
        }
    }

    for (const agentKey of HIDDEN_AGENT_KEYS) {
        const block = projectRaw[agentKey];
        if (!isPlainObject(block)) continue;
        const removed: string[] = [];
        for (const field of AGENT_ESCALATION_FIELDS) {
            if (field in block) {
                delete block[field];
                removed.push(field);
            }
        }
        if (removed.length > 0) {
            warnings.push(
                `Ignoring ${agentKey}.${removed.join("/")} from project config ` +
                    "(security: a repository cannot reprogram or re-permission hidden agents).",
            );
        }
        if ("disable" in block) {
            delete block.disable;
            warnings.push(
                `Ignoring ${agentKey}.disable from project config (security: only user-level config may enable or disable hidden agents; a repository cannot reactivate an agent the user turned off).`,
            );
        }
    }

    return warnings;
}

/**
 * The merged threshold is always rewritten from the trusted value plus any project raises, so
 * `mergedRaw` never carries a project value the sanitizer did not recognize.
 */
export function constrainProjectThresholdOverrides(args: {
    mergedRaw: Record<string, unknown>;
    projectRaw: Record<string, unknown>;
    trustedBaseConfig: {
        execute_threshold_percentage?: unknown;
        execute_threshold_tokens?: unknown;
    };
}): string[] {
    const warnings: string[] = [];
    const basePercentage = normalizeTrustedPercentageThresholds(
        args.trustedBaseConfig.execute_threshold_percentage,
    );
    const baseTokens = normalizeTrustedTokenThresholds(
        args.trustedBaseConfig.execute_threshold_tokens,
    );

    if ("execute_threshold_percentage" in args.projectRaw) {
        const projectValue = args.projectRaw.execute_threshold_percentage;
        const constrained = clonePercentageThresholds(basePercentage);

        if (isValidPercentageThreshold(projectValue)) {
            constrained.defaultValue = Math.max(basePercentage.defaultValue, projectValue);
            for (const [modelKey, threshold] of basePercentage.overrides) {
                const raisedThreshold = Math.max(threshold, projectValue);
                if (raisedThreshold === constrained.defaultValue) {
                    constrained.overrides.delete(modelKey);
                } else {
                    constrained.overrides.set(modelKey, raisedThreshold);
                }
            }
            if (percentageThresholdsEqual(constrained, basePercentage)) {
                warnings.push(
                    makeProjectThresholdWarning(
                        "execute_threshold_percentage",
                        PERCENTAGE_THRESHOLD_REASON,
                    ),
                );
            }
        } else if (isPlainObject(projectValue)) {
            if ("default" in projectValue) {
                if (!isValidPercentageThreshold(projectValue.default)) {
                    warnings.push(
                        makeProjectThresholdWarning(
                            "execute_threshold_percentage.default",
                            INVALID_THRESHOLD_REASON,
                        ),
                    );
                } else if (projectValue.default > basePercentage.defaultValue) {
                    constrained.defaultValue = projectValue.default;
                } else {
                    warnings.push(
                        makeProjectThresholdWarning(
                            "execute_threshold_percentage.default",
                            PERCENTAGE_THRESHOLD_REASON,
                        ),
                    );
                }
            }

            for (const [modelKey, rawValue] of Object.entries(projectValue)) {
                if (modelKey === "default") continue;
                if (!isValidPercentageThreshold(rawValue)) {
                    warnings.push(
                        makeProjectThresholdWarning(
                            `execute_threshold_percentage.${modelKey}`,
                            INVALID_THRESHOLD_REASON,
                        ),
                    );
                    continue;
                }
                const baseValue = resolveTrustedThreshold(basePercentage, modelKey);
                if (rawValue > baseValue) {
                    if (rawValue === constrained.defaultValue) {
                        constrained.overrides.delete(modelKey);
                    } else {
                        constrained.overrides.set(modelKey, rawValue);
                    }
                } else {
                    warnings.push(
                        makeProjectThresholdWarning(
                            `execute_threshold_percentage.${modelKey}`,
                            PERCENTAGE_THRESHOLD_REASON,
                        ),
                    );
                }
            }
        } else {
            warnings.push(
                makeProjectThresholdWarning(
                    "execute_threshold_percentage",
                    INVALID_THRESHOLD_REASON,
                ),
            );
        }

        setMergedPercentageThreshold(args.mergedRaw, constrained);
    }

    if ("execute_threshold_tokens" in args.projectRaw) {
        const projectValue = args.projectRaw.execute_threshold_tokens;
        const constrained = cloneTokenThresholds(baseTokens);

        if (isPlainObject(projectValue)) {
            if ("default" in projectValue) {
                if (!isValidTokenThreshold(projectValue.default)) {
                    warnings.push(
                        makeProjectThresholdWarning(
                            "execute_threshold_tokens.default",
                            INVALID_THRESHOLD_REASON,
                        ),
                    );
                } else if (baseTokens.defaultValue === undefined) {
                    warnings.push(
                        makeProjectThresholdWarning(
                            "execute_threshold_tokens.default",
                            TOKEN_THRESHOLD_INTRODUCTION_REASON,
                        ),
                    );
                } else if (projectValue.default > baseTokens.defaultValue) {
                    constrained.defaultValue = projectValue.default;
                } else {
                    warnings.push(
                        makeProjectThresholdWarning(
                            "execute_threshold_tokens.default",
                            TOKEN_THRESHOLD_REASON,
                        ),
                    );
                }
            }

            for (const [modelKey, rawValue] of Object.entries(projectValue)) {
                if (modelKey === "default") continue;
                if (!isValidTokenThreshold(rawValue)) {
                    warnings.push(
                        makeProjectThresholdWarning(
                            `execute_threshold_tokens.${modelKey}`,
                            INVALID_THRESHOLD_REASON,
                        ),
                    );
                    continue;
                }
                const baseValue = resolveTrustedThreshold(baseTokens, modelKey);
                if (baseValue === undefined) {
                    warnings.push(
                        makeProjectThresholdWarning(
                            `execute_threshold_tokens.${modelKey}`,
                            TOKEN_THRESHOLD_INTRODUCTION_REASON,
                        ),
                    );
                    continue;
                }
                if (rawValue > baseValue) {
                    if (rawValue === constrained.defaultValue) {
                        constrained.overrides.delete(modelKey);
                    } else {
                        constrained.overrides.set(modelKey, rawValue);
                    }
                } else {
                    warnings.push(
                        makeProjectThresholdWarning(
                            `execute_threshold_tokens.${modelKey}`,
                            TOKEN_THRESHOLD_REASON,
                        ),
                    );
                }
            }
        } else {
            warnings.push(
                makeProjectThresholdWarning("execute_threshold_tokens", INVALID_THRESHOLD_REASON),
            );
        }

        setMergedTokenThreshold(args.mergedRaw, constrained);
    }

    return warnings;
}
