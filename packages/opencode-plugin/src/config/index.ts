import { existsSync, readFileSync } from "node:fs";

import {
    detectConfigFile,
    isPrototypePollutionKey,
    parseConfigJsonc,
} from "../shared/jsonc-parser";
import { setOutputReserveConfig } from "../shared/models-dev-cache";
import type { PromptSurfaceConfig } from "../shared/prompt-surface";
import { isRecord } from "../shared/record-type-guard";
import { setWindowOverlayPath } from "../shared/window-geometry";
import { isCompactionEnabled, migrateLegacyAgentEnabledInMemory } from "./agent-disable";
import { eidnaraProjectConfigBasePath, eidnaraUserConfigBasePath } from "./config-paths";
import type { LoadOutcome } from "./load-outcome";
import {
    constrainProjectThresholdOverrides,
    stripUnsafeProjectConfigFields,
} from "./project-security";
import { pruneNestedConfigLeaf } from "./prune-config-leaf";
import { type EidnaraConfig, EidnaraConfigSchema, REMOVED_CONFIG_KEYS } from "./schema/eidnara";
import { resolveTransformMode } from "./transform-mode";
import { substituteConfigVariables } from "./variable";

export type { LoadOutcome } from "./load-outcome";

export interface EidnaraPluginConfig extends EidnaraConfig {
    disabled_hooks?: string[];
    command?: Record<
        string,
        {
            template: string;
            description?: string;
            agent?: string;
            model?: string;
            subtask?: boolean;
        }
    >;
}

function getUserConfigBasePath(): string {
    return eidnaraUserConfigBasePath();
}

function getProjectConfigBasePath(directory: string): string {
    return eidnaraProjectConfigBasePath(directory);
}

interface LoadedConfigFile {
    config: Record<string, unknown>;
    /** The loader prefixes {env:} and {file:} substitution warnings with the config path. */
    warnings: string[];
}

export interface LoadResultDetailed {
    config: EidnaraPluginConfig & { configWarnings?: string[] };
    /** The loader captures USER-tier defaults and overrides before merging project routing. */
    registrationPromptSurface: PromptSurfaceConfig;
    loadOutcome: LoadOutcome;
    sources: {
        userConfig: LoadOutcome;
        projectConfig: LoadOutcome;
    };
    substitutionFailures: Array<{ keyPath: string; source: "user" | "project"; message: string }>;
    recoveredTopLevelKeys: string[];
}

interface LoadedConfigFileDetailed extends LoadedConfigFile {
    outcome: LoadOutcome;
    source: "user" | "project";
    /**
     * The `{env:}`/`{file:}` subset of `warnings`. A rejected prototype-pollution key in the same
     * file sets `outcome` to `schema-recovery`, so `outcome` alone cannot identify these failures.
     */
    substitutionWarnings: string[];
}

/**
 * Ancestor key names are withheld because substitution runs on the raw text, so a parent key can
 * carry a resolved secret. The rejected key itself is always one of the fixed prototype-pollution names.
 */
function describeRejectedKeyPath(path: readonly (string | number)[]): string {
    const key = String(path.at(-1) ?? "");
    return path.length > 1 ? `"${key}" at depth ${path.length}` : `"${key}"`;
}

function loadConfigFileDetailed(
    configPath: string,
    source: "user" | "project",
): LoadedConfigFileDetailed | null {
    if (!existsSync(configPath)) {
        return null;
    }

    let rawText: string;
    try {
        rawText = readFileSync(configPath, "utf-8");
    } catch (error) {
        return {
            config: {},
            warnings: [
                `${configPath}: failed to read config: ${error instanceof Error ? error.message : String(error)}`,
            ],
            outcome: "project-file-io-error",
            source,
            substitutionWarnings: [],
        };
    }

    try {
        const substituted = substituteConfigVariables({
            text: rawText,
            configPath,
            isProjectConfig: source === "project",
        });
        const rejectedKeyPaths: (string | number)[][] = [];
        const parsed: unknown = parseConfigJsonc(substituted.text, {
            onRejectedKey: (path) => rejectedKeyPaths.push([...path]),
        });
        // The generic parser returns whatever JSON value the file holds; a `null`, array, or scalar top level would throw inside `parsePluginConfig`, outside this try.
        if (!isRecord(parsed)) {
            throw new Error(
                `config top level must be a JSON object, got ${parsed === null ? "null" : Array.isArray(parsed) ? "array" : typeof parsed}`,
            );
        }
        const config: Record<string, unknown> = parsed;
        const prefix = (warning: string) => `${configPath}: ${warning}`;
        const substitutionWarnings = substituted.warnings.map(prefix);
        const unsafeKeyWarnings = rejectedKeyPaths.map((path) =>
            prefix(
                `Ignored unsafe config key ${describeRejectedKeyPath(path)} (security: prototype-pollution keys are not allowed).`,
            ),
        );
        return {
            config,
            warnings: [...substitutionWarnings, ...unsafeKeyWarnings],
            outcome:
                rejectedKeyPaths.length > 0
                    ? "schema-recovery"
                    : substitutionWarnings.length > 0
                      ? "substitution-failure"
                      : "ok",
            source,
            substitutionWarnings,
        };
    } catch (error) {
        return {
            config: {},
            warnings: [
                `${configPath}: failed to load config: ${error instanceof Error ? error.message : String(error)}`,
            ],
            outcome: "project-file-parse-error",
            source,
            substitutionWarnings: [],
        };
    }
}

/**
 * The loader merges raw JSON before Zod parsing so defaults do not become overrides.
 *
 * The merge recursively merges plain objects and atomically replaces arrays, primitives, and `null`.
 * The merge union-merges `disabled_hooks` so user and project configs can both contribute hook IDs.
 * element-wise.
 *
 * other's entries.
 */
function defineOwnConfigValue(target: Record<string, unknown>, key: string, value: unknown): void {
    Object.defineProperty(target, key, {
        value,
        enumerable: true,
        configurable: true,
        writable: true,
    });
}

function deepMergeRawConfig(
    base: Record<string, unknown>,
    override: Record<string, unknown>,
): Record<string, unknown> {
    const result: Record<string, unknown> = {};
    for (const key of Object.keys(base)) {
        if (isPrototypePollutionKey(key)) continue;
        defineOwnConfigValue(result, key, base[key]);
    }

    for (const key of Object.keys(override)) {
        if (isPrototypePollutionKey(key)) continue;
        const baseVal = Object.hasOwn(base, key) ? base[key] : undefined;
        const overrideVal = override[key];
        let mergedValue: unknown;
        if (
            baseVal !== null &&
            typeof baseVal === "object" &&
            !Array.isArray(baseVal) &&
            overrideVal !== null &&
            typeof overrideVal === "object" &&
            !Array.isArray(overrideVal)
        ) {
            mergedValue = deepMergeRawConfig(
                baseVal as Record<string, unknown>,
                overrideVal as Record<string, unknown>,
            );
        } else if (
            key === "disabled_hooks" &&
            Array.isArray(baseVal) &&
            Array.isArray(overrideVal)
        ) {
            mergedValue = [...new Set([...baseVal, ...overrideVal])];
        } else {
            mergedValue = overrideVal;
        }
        defineOwnConfigValue(result, key, mergedValue);
    }
    return result;
}

/**
 * Warning rendering never exposes values resolved by `{env:...}` or `{file:...}` substitution.
 *
 * Object keys are withheld because substitution runs on the raw text, so a key can hold a resolved secret as readily as a value.
 */
function redactConfigValue(value: unknown): string {
    if (value === undefined) return "<missing>";
    if (value === null) return "null";
    if (typeof value === "string")
        return `string, ${value.length} char${value.length === 1 ? "" : "s"}`;
    if (typeof value === "number") return `number ${value}`;
    if (typeof value === "boolean") return `boolean ${value}`;
    if (Array.isArray(value)) return `array, ${value.length} item${value.length === 1 ? "" : "s"}`;
    if (typeof value === "object") {
        const count = Object.keys(value as Record<string, unknown>).length;
        return `object with ${count} key${count === 1 ? "" : "s"}`;
    }
    return typeof value;
}

function parsePluginConfig(
    rawConfig: Record<string, unknown>,
    recoveredTopLevelKeys: string[] = [],
): EidnaraPluginConfig & { configWarnings?: string[] } {
    // The loader migrates legacy `<agent>.enabled` keys before Zod parsing so opt-outs become `disable: true` without running `doctor`.
    const preMigrationWarnings: string[] = [];
    const migrated = migrateLegacyAgentEnabledInMemory(rawConfig, preMigrationWarnings);
    const parsed = EidnaraConfigSchema.safeParse(migrated);
    const disabledHooks = Array.isArray(rawConfig.disabled_hooks)
        ? rawConfig.disabled_hooks.filter((value): value is string => typeof value === "string")
        : undefined;
    const command =
        typeof rawConfig.command === "object" && rawConfig.command !== null
            ? (rawConfig.command as EidnaraPluginConfig["command"])
            : undefined;

    if (parsed.success) {
        return {
            ...parsed.data,
            disabled_hooks: disabledHooks,
            command,
            ...(preMigrationWarnings.length > 0 ? { configWarnings: preMigrationWarnings } : {}),
        };
    }

    const defaults = EidnaraConfigSchema.parse({});
    const warnings: string[] = [];

    const errorPaths = new Set<string>();
    // The validator excludes generic Zod messages so warnings retain actionable validation reasons.
    // The loader surfaces custom Zod messages as config warnings.
    const customMessagesByKey = new Map<string, string>();
    // `issuePathsByKey` lets recovery prune invalid nested leaves without deleting their parent blocks.
    const issuePathsByKey = new Map<string, PropertyKey[][]>();
    const GENERIC_ZOD_PREFIXES = ["Too big", "Too small", "Invalid input", "Invalid", "Expected"];
    for (const issue of parsed.error.issues) {
        const topKey = issue.path[0];
        if (topKey !== undefined) {
            const key = String(topKey);
            errorPaths.add(key);
            const paths = issuePathsByKey.get(key) ?? [];
            paths.push([...issue.path]);
            issuePathsByKey.set(key, paths);
            const msg = issue.message;
            if (msg && !GENERIC_ZOD_PREFIXES.some((p) => msg.startsWith(p))) {
                if (!customMessagesByKey.has(key)) {
                    customMessagesByKey.set(key, msg);
                }
            }
        }
    }

    const patched: Record<string, unknown> = { ...rawConfig };
    for (const key of errorPaths) {
        recoveredTopLevelKeys.push(key);

        // Recovery prunes invalid nested leaves from object-valued keys and preserves valid siblings.
        // Preserving valid siblings retains `memory.auto_search` and `memory.git_commit_indexing` settings.
        // For `historian` and `sidekick`, pruning the invalid leaf keeps the user's `disable` and `model`;
        // discarding the whole block would let a project reset them by supplying one invalid leaf.
        // The recovery code deletes the whole key when the issue targets that key or its value is not a prunable object.
        const issuePaths = issuePathsByKey.get(key) ?? [];
        const rawValue = rawConfig[key];
        const allNested =
            issuePaths.length > 0 &&
            issuePaths.every((p) => p.length >= 2) &&
            typeof rawValue === "object" &&
            rawValue !== null &&
            !Array.isArray(rawValue);
        if (allNested) {
            let prunedBlock: Record<string, unknown> = {
                ...(rawValue as Record<string, unknown>),
            };
            const prunedLeaves: string[] = [];
            for (const p of issuePaths) {
                // `p` is the full Zod issue path; recovery removes its deepest invalid leaf.
                // Recovery removes the deepest invalid leaf rather than `p[1]`.
                // For `memory.git_commit_indexing.since_days`, recovery removes only `since_days`.
                // For `memory.git_commit_indexing.since_days`, recovery preserves sibling fields such as `enabled: false`.
                const relative = p.slice(1);
                const result = pruneNestedConfigLeaf(prunedBlock, relative);
                if (result) {
                    prunedBlock = result.block;
                    prunedLeaves.push(result.removed);
                }
            }
            patched[key] = prunedBlock;
            const reason = customMessagesByKey.get(key);
            warnings.push(
                `"${key}": invalid nested field(s) ${prunedLeaves.map((l) => `"${l}"`).join(", ")}, using defaults for those.${reason ? ` ${reason}` : ""}`,
            );
            continue;
        }

        // `redactConfigValue` reports type and length, not resolved values, because `{env:...}` and `{file:...}` substitutions may expand secrets into `rawConfig`.
        delete patched[key];
        // Optional blocks such as `historian` have no default and are omitted after validation fails.
        const defaultVal = (defaults as unknown as Record<string, unknown>)[key];
        const reason = customMessagesByKey.get(key);
        const fallback =
            defaultVal === undefined
                ? "omitting it"
                : `using default ${JSON.stringify(defaultVal)}`;
        warnings.push(
            `"${key}": invalid value (${redactConfigValue(rawConfig[key])}), ${fallback}.${reason ? ` ${reason}` : ""}`,
        );
    }

    // `patched` derives from `rawConfig` by deleting or pruning keys, so any legacy `enabled` field the retry migrates was already migrated and reported by the first pass.
    const retryMigrated = migrateLegacyAgentEnabledInMemory(patched, []);
    const retryParsed = EidnaraConfigSchema.safeParse(retryMigrated);
    if (retryParsed.success) {
        return {
            ...retryParsed.data,
            disabled_hooks: disabledHooks,
            command,
            configWarnings: [...preMigrationWarnings, ...warnings],
        };
    }

    warnings.push("Config recovery failed, using all defaults.");
    return {
        ...defaults,
        disabled_hooks: disabledHooks,
        command,
        configWarnings: [...preMigrationWarnings, ...warnings],
    };
}

export function loadPluginConfig(
    directory: string,
): EidnaraPluginConfig & { configWarnings?: string[] } {
    return loadPluginConfigDetailed(directory).config;
}

function hasUserTierExplicitDaemonConfig(config: Record<string, unknown> | undefined): boolean {
    const { subc } = config ?? {};
    if (typeof subc !== "object" || subc === null || Array.isArray(subc)) return false;
    const connectionFile = (subc as Record<string, unknown>).connection_file;
    return typeof connectionFile === "string" && connectionFile.trim().length > 0;
}

function collectEmptyStringPaths(value: unknown, prefix = ""): string[] {
    if (typeof value === "string") {
        return value === "" && prefix ? [prefix] : [];
    }
    if (Array.isArray(value) || value === null || typeof value !== "object") {
        return [];
    }

    const paths: string[] = [];
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
        const nextPrefix = prefix ? `${prefix}.${key}` : key;
        paths.push(...collectEmptyStringPaths(child, nextPrefix));
    }
    return paths;
}

function bindSubstitutionFailures(
    loaded: LoadedConfigFileDetailed | null,
): Array<{ keyPath: string; source: "user" | "project"; message: string }> {
    if (!loaded || loaded.substitutionWarnings.length === 0) {
        return [];
    }

    const emptyPaths = collectEmptyStringPaths(loaded.config);
    return loaded.substitutionWarnings.map((message) => {
        const matchedPath = emptyPaths.find((path) => {
            const tail = path.split(".").at(-1) ?? path;
            return message.includes(path) || message.toLowerCase().includes(tail.toLowerCase());
        });
        return { keyPath: matchedPath ?? "<unknown>", source: loaded.source, message };
    });
}

/** Zod strips removed keys silently; this names them so users learn the key no longer does anything. */
function removedKeyWarnings(raw: Record<string, unknown>): string[] {
    return REMOVED_CONFIG_KEYS.filter((key) => Object.hasOwn(raw, key)).map(
        (key) => `"${key}" is no longer a configuration key and is ignored.`,
    );
}

function withSchemaRecovery(outcome: LoadOutcome, recoveredKeys: readonly string[]): LoadOutcome {
    if (recoveredKeys.length === 0) return outcome;
    return outcome === "ok" || outcome === "substitution-failure" ? "schema-recovery" : outcome;
}

function combinedOutcome(args: {
    sources: LoadResultDetailed["sources"];
    substitutionFailures: LoadResultDetailed["substitutionFailures"];
    recoveredTopLevelKeys: string[];
}): LoadOutcome {
    const sourceOutcomes = Object.values(args.sources);
    if (sourceOutcomes.includes("project-file-parse-error")) return "project-file-parse-error";
    if (sourceOutcomes.includes("project-file-io-error")) return "project-file-io-error";
    // A rejected prototype-pollution key never reaches Zod, so it appears only as a source outcome, not in `recoveredTopLevelKeys`.
    if (sourceOutcomes.includes("schema-recovery") || args.recoveredTopLevelKeys.length > 0) {
        return "schema-recovery";
    }
    if (args.substitutionFailures.length > 0) return "substitution-failure";
    return "ok";
}

export function loadPluginConfigDetailed(directory: string): LoadResultDetailed {
    const userDetected = detectConfigFile(getUserConfigBasePath());
    const projectDetected = detectConfigFile(getProjectConfigBasePath(directory));

    const userLoaded =
        userDetected.format !== "none" ? loadConfigFileDetailed(userDetected.path, "user") : null;
    const projectLoaded =
        projectDetected.format !== "none"
            ? loadConfigFileDetailed(projectDetected.path, "project")
            : null;

    const allWarnings: string[] = [];
    let mergedRaw: Record<string, unknown> = {};
    const userRecoveredTopLevelKeys: string[] = [];
    const trustedBaseConfig = parsePluginConfig(
        userLoaded?.config ?? {},
        userRecoveredTopLevelKeys,
    );

    if (userLoaded) {
        allWarnings.push(...userLoaded.warnings.map((w) => `[user config] ${w}`));
        allWarnings.push(...removedKeyWarnings(userLoaded.config).map((w) => `[user config] ${w}`));
        mergedRaw = deepMergeRawConfig(mergedRaw, userLoaded.config);
    }

    if (projectLoaded) {
        allWarnings.push(...projectLoaded.warnings.map((w) => `[project config] ${w}`));
        allWarnings.push(
            ...removedKeyWarnings(projectLoaded.config).map((w) => `[project config] ${w}`),
        );
        const projectRaw = { ...projectLoaded.config };
        for (const warning of stripUnsafeProjectConfigFields(projectRaw)) {
            allWarnings.push(`[project config] ${warning}`);
        }
        mergedRaw = deepMergeRawConfig(mergedRaw, projectRaw);
        for (const warning of constrainProjectThresholdOverrides({
            mergedRaw,
            projectRaw,
            trustedBaseConfig,
        })) {
            allWarnings.push(`[project config] ${warning}`);
        }
    }

    const mergedRecoveredTopLevelKeys: string[] = [];
    const config = parsePluginConfig(mergedRaw, mergedRecoveredTopLevelKeys);
    setOutputReserveConfig(config.output_reserve);
    setWindowOverlayPath(config.models?.window_overlay_path);
    if (userLoaded && projectLoaded) {
        // A project override can hide an invalid user field from the merged parse.
        // The user-tier warning is kept unless the merged parse emitted the same warning.
        const mergedWarnings = new Set(config.configWarnings ?? []);
        for (const warning of trustedBaseConfig.configWarnings ?? []) {
            if (!mergedWarnings.has(warning)) allWarnings.push(`[user config] ${warning}`);
        }
    }
    if (config.configWarnings?.length) {
        allWarnings.push(
            ...config.configWarnings.map((w) => {
                if (userLoaded && projectLoaded) return `[config] ${w}`;
                if (userLoaded) return `[user config] ${w}`;
                return `[project config] ${w}`;
            }),
        );
    }

    const resolvedTransformMode = resolveTransformMode({
        configured: config.transform_mode,
        userTierConfiguredRust: userLoaded?.config?.transform_mode === "rust",
        userTierHasExplicitDaemon: hasUserTierExplicitDaemonConfig(userLoaded?.config),
        compactionEnabled: isCompactionEnabled(config),
    });
    config.transform_mode = resolvedTransformMode.mode;
    allWarnings.push(...resolvedTransformMode.warnings.map((warning) => `[config] ${warning}`));

    if (allWarnings.length > 0) {
        config.configWarnings = allWarnings;
    } else if ("configWarnings" in config) {
        config.configWarnings = undefined;
    }

    const substitutionFailures = [
        ...bindSubstitutionFailures(userLoaded),
        ...bindSubstitutionFailures(projectLoaded),
    ];
    const sources: LoadResultDetailed["sources"] = {
        userConfig: userLoaded
            ? withSchemaRecovery(userLoaded.outcome, userRecoveredTopLevelKeys)
            : "ok",
        projectConfig: projectLoaded?.outcome ?? "ok",
    };
    const recoveredTopLevelKeys = [
        ...new Set([...userRecoveredTopLevelKeys, ...mergedRecoveredTopLevelKeys]),
    ];

    return {
        config,
        registrationPromptSurface: trustedBaseConfig.prompt_surface,
        loadOutcome: combinedOutcome({ sources, substitutionFailures, recoveredTopLevelKeys }),
        sources,
        substitutionFailures,
        recoveredTopLevelKeys,
    };
}
