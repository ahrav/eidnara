import "@eidnara/opencode/config/prune-config-leaf";
import { existsSync, readFileSync } from "node:fs";
import { migrateLegacyAgentEnabledInMemory } from "@eidnara/opencode/config/agent-disable";
import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
} from "@eidnara/opencode/config/config-paths";
import type { LoadOutcome } from "@eidnara/opencode/config/load-outcome";
import {
    constrainProjectThresholdOverrides,
    stripUnsafeProjectConfigFields,
} from "@eidnara/opencode/config/project-security";
import { pruneNestedConfigLeaf } from "@eidnara/opencode/config/prune-config-leaf";
import {
    type EidnaraConfig,
    EidnaraConfigSchema,
    REMOVED_CONFIG_KEYS,
} from "@eidnara/opencode/config/schema/eidnara";
import { redactConfigIssuePath } from "@eidnara/opencode/config/schema/issue-path";
import { substituteConfigVariables } from "@eidnara/opencode/config/variable";
import { isPrototypePollutionKey, parseConfigJsonc } from "@eidnara/opencode/shared/jsonc-parser";
import { setOutputReserveConfig } from "@eidnara/opencode/shared/models-dev-cache";
import type { PromptSurfaceConfig } from "@eidnara/opencode/shared/prompt-surface";
import { setWindowOverlayPath } from "@eidnara/opencode/shared/window-geometry";

export type { LoadOutcome } from "@eidnara/opencode/config/load-outcome";

export interface LoadPiConfigOptions {
    cwd?: string;
}

export interface LoadPiConfigResult {
    config: EidnaraConfig;
    /** registrationPromptSurface contains USER-tier defaults and overrides before project routing merges. */
    registrationPromptSurface: PromptSurfaceConfig;
    warnings: string[];
    loadedFromPaths: string[];
}

export interface LoadPiConfigResultDetailed extends LoadPiConfigResult {
    loadOutcome: LoadOutcome;
    sources: {
        userConfig: LoadOutcome;
        projectConfig: LoadOutcome;
    };
    substitutionFailures: Array<{
        keyPath: string;
        source: "user" | "project";
        message: string;
    }>;
    recoveredTopLevelKeys: string[];
}

interface LoadedConfigFile {
    path: string;
    scope: "user" | "project";
    config: Record<string, unknown>;
    warnings: string[];
    loadOutcome: LoadOutcome;
}

function getProjectConfigPaths(cwd: string): string[] {
    const basePath = eidnaraProjectConfigBasePath(cwd);
    return [`${basePath}.jsonc`, `${basePath}.json`];
}

function getUserConfigPaths(): string[] {
    const basePath = eidnaraUserConfigBasePath();
    return [`${basePath}.jsonc`, `${basePath}.json`];
}

function resolveFirstExisting(paths: string[]): string | undefined {
    return paths.find((path) => existsSync(path));
}

function loadConfigFile(path: string, scope: "user" | "project"): LoadedConfigFile | null {
    try {
        const rawText = readFileSync(path, "utf-8");
        const substituted = substituteConfigVariables({
            text: rawText,
            configPath: path,
            // Project configs cannot expand `{env:}` or `{file:}` tokens because they may expose secrets.
            // Project configs cannot expand `{env:}` or `{file:}` tokens because they may expose secrets.
            isProjectConfig: scope === "project",
        });
        const rejectedKeyPaths: string[] = [];
        const parsed = parseConfigJsonc<unknown>(substituted.text, {
            onRejectedKey: (keyPath) => rejectedKeyPaths.push(keyPath.join(".")),
        });
        // Reject non-object roots because `removedKeyWarnings` and the raw merge index them by key.
        if (!isPlainObject(parsed)) {
            throw new Error(`config root must be a JSON object, got ${redactConfigValue(parsed)}`);
        }
        const config = parsed;
        const unsafeKeyWarnings = rejectedKeyPaths.map(
            (keyPath) =>
                `Ignored unsafe config key "${keyPath}" (security: prototype-pollution keys are not allowed).`,
        );
        return {
            path,
            scope,
            config,
            warnings: [...substituted.warnings, ...unsafeKeyWarnings].map(
                (warning) => `${path}: ${warning}`,
            ),
            loadOutcome:
                rejectedKeyPaths.length > 0
                    ? "schema-recovery"
                    : substituted.warnings.length > 0
                      ? "substitution-failure"
                      : "ok",
        };
    } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return {
            path,
            scope,
            config: {},
            warnings: [`${path}: failed to load config: ${message}; using defaults for this file.`],
            loadOutcome:
                typeof (error as { code?: unknown }).code === "string"
                    ? "project-file-io-error"
                    : "project-file-parse-error",
        };
    }
}

function redactConfigValue(value: unknown): string {
    if (value === undefined) return "<missing>";
    if (value === null) return "null";
    if (typeof value === "string") {
        return `string, ${value.length} char${value.length === 1 ? "" : "s"}`;
    }
    if (typeof value === "number") return `number ${value}`;
    if (typeof value === "boolean") return `boolean ${value}`;
    if (Array.isArray(value)) return `array, ${value.length} item${value.length === 1 ? "" : "s"}`;
    if (typeof value === "object") {
        const keys = Object.keys(value as Record<string, unknown>);
        return `object with keys [${keys.join(", ")}]`;
    }
    return typeof value;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function defineOwnConfigValue(target: Record<string, unknown>, key: string, value: unknown): void {
    Object.defineProperty(target, key, {
        value,
        enumerable: true,
        configurable: true,
        writable: true,
    });
}

function mergeRawConfigs(
    base: Record<string, unknown>,
    override: Record<string, unknown>,
): Record<string, unknown> {
    const merged: Record<string, unknown> = {};
    for (const key of Object.keys(base)) {
        if (isPrototypePollutionKey(key)) continue;
        defineOwnConfigValue(merged, key, base[key]);
    }

    for (const key of Object.keys(override)) {
        if (isPrototypePollutionKey(key)) continue;
        const overrideValue = override[key];
        const baseValue = Object.hasOwn(base, key) ? base[key] : undefined;
        const mergedValue =
            isPlainObject(baseValue) && isPlainObject(overrideValue)
                ? mergeRawConfigs(baseValue, overrideValue)
                : overrideValue;
        defineOwnConfigValue(merged, key, mergedValue);
    }

    return merged;
}

/** Zod strips removed keys silently; this names them so users learn the key no longer does anything. */
function removedKeyWarnings(raw: Record<string, unknown>): string[] {
    return REMOVED_CONFIG_KEYS.filter((key) => Object.hasOwn(raw, key)).map(
        (key) => `"${key}" is no longer a configuration key and is ignored.`,
    );
}

/** Omitting keys absent from `userRaw` preserves per-leaf pruning during project-config recovery. */
function userTierFallbackFor(
    projectRaw: Record<string, unknown>,
    userRaw: Record<string, unknown> | undefined,
    trustedBaseConfig: EidnaraConfig,
): Map<string, unknown> {
    const trusted = trustedBaseConfig as unknown as Record<string, unknown>;
    const fallback = new Map<string, unknown>();
    if (userRaw === undefined) return fallback;
    for (const key of Object.keys(projectRaw)) {
        if (Object.hasOwn(userRaw, key)) fallback.set(key, trusted[key]);
    }
    return fallback;
}

interface ParsePiConfigOptions {
    recoveredTopLevelKeys?: string[];
    /** Stores user-tier values used instead of schema defaults when recovery rejects merged top-level keys. */
    userTierFallback?: ReadonlyMap<string, unknown>;
}

function parsePiConfig(
    rawConfig: Record<string, unknown>,
    { recoveredTopLevelKeys = [], userTierFallback }: ParsePiConfigOptions = {},
): {
    config: EidnaraConfig;
    warnings: string[];
} {
    const preMigrationWarnings: string[] = [];
    const migrated = migrateLegacyAgentEnabledInMemory(rawConfig, preMigrationWarnings);
    const parsed = EidnaraConfigSchema.safeParse(migrated);
    if (parsed.success) {
        return { config: parsed.data, warnings: preMigrationWarnings };
    }

    const defaults = EidnaraConfigSchema.parse({});
    const errorPaths = new Set<string>();
    // Validation retains full error paths for each top-level key.
    // Full error paths let recovery remove invalid nested leaves without deleting their containing block.
    const issuePathsByKey = new Map<string, PropertyKey[][]>();
    for (const issue of parsed.error.issues) {
        const topKey = issue.path[0];
        if (topKey !== undefined) {
            const key = String(topKey);
            errorPaths.add(key);
            const paths = issuePathsByKey.get(key) ?? [];
            paths.push([...issue.path]);
            issuePathsByKey.set(key, paths);
        }
    }

    const patched: Record<string, unknown> = { ...migrated };
    const warnings: string[] = [...preMigrationWarnings];

    for (const key of errorPaths) {
        recoveredTopLevelKeys.push(key);
        const isAgentConfig = key === "historian" || key === "sidekick";

        // A project config key with a user-tier fallback restores that fallback instead
        // of forcing the schema default.
        if (userTierFallback?.has(key)) {
            const fallback = userTierFallback.get(key);
            if (fallback === undefined) {
                delete patched[key];
            } else {
                patched[key] = fallback;
            }
            warnings.push(
                `"${key}": invalid value (${redactConfigValue(rawConfig[key])}) after merging the project config, keeping the user config's ${key} settings. Check the project's eidnara.jsonc.`,
            );
            continue;
        }

        if (isAgentConfig) {
            delete patched[key];
            warnings.push(
                `"${key}": invalid agent configuration, ignoring. Check your eidnara.jsonc.`,
            );
            continue;
        }

        // For object-valued keys, recovery prunes only invalid nested leaves and keeps valid siblings.
        // Recovery keeps valid `memory` siblings when one nested field is invalid.
        // Recovery deletes the entire key only when the error is at that key or its value is not an object.
        const issuePaths = issuePathsByKey.get(key) ?? [];
        const rawValue = migrated[key];
        const allNested =
            issuePaths.length > 0 &&
            issuePaths.every((p) => p.length >= 2) &&
            typeof rawValue === "object" &&
            rawValue !== null &&
            !Array.isArray(rawValue);
        if (allNested) {
            let prunedBlock: Record<string, unknown> | undefined = {
                ...(rawValue as Record<string, unknown>),
            };
            const prunedLeaves: string[] = [];
            for (const p of issuePaths) {
                // Recovery prunes the deepest invalid leaf so valid siblings remain.
                // Recovery preserves a sibling `enabled: false`.
                const relative = p.slice(1);
                const result = pruneNestedConfigLeaf(prunedBlock, relative);
                if (result) {
                    prunedBlock = result.block;
                    // The rendered leaf omits `key`, which the warning names separately.
                    prunedLeaves.push(
                        redactConfigIssuePath([key, ...result.removed])
                            .slice(1)
                            .join("."),
                    );
                    continue;
                }
                // A missing required leaf has nothing to prune, so the whole block goes.
                prunedBlock = undefined;
                break;
            }
            if (prunedBlock !== undefined) {
                patched[key] = prunedBlock;
                warnings.push(
                    `"${key}": invalid nested field(s) ${prunedLeaves.map((l) => `"${l}"`).join(", ")}, using defaults for those.`,
                );
                continue;
            }
        }

        delete patched[key];
        const defaultValue = (defaults as unknown as Record<string, unknown>)[key];
        warnings.push(
            `"${key}": invalid value (${redactConfigValue(rawConfig[key])}), using default ${JSON.stringify(defaultValue)}.`,
        );
    }

    const retryParsed = EidnaraConfigSchema.safeParse(patched);
    if (retryParsed.success) {
        return { config: retryParsed.data, warnings };
    }

    warnings.push("Config recovery failed, using all defaults.");
    return { config: defaults, warnings };
}

export function loadPiConfig(opts: LoadPiConfigOptions = {}): LoadPiConfigResult {
    const cwd = opts.cwd ?? process.cwd();
    const loadedFiles: LoadedConfigFile[] = [];
    const warnings: string[] = [];

    const projectPath = resolveFirstExisting(getProjectConfigPaths(cwd));
    if (projectPath) {
        const loaded = loadConfigFile(projectPath, "project");
        if (loaded) loadedFiles.push(loaded);
    }

    const userPath = resolveFirstExisting(getUserConfigPaths());
    if (userPath) {
        const loaded = loadConfigFile(userPath, "user");
        if (loaded) loadedFiles.push(loaded);
    }

    let rawConfig: Record<string, unknown> = {};
    const mergeFiles = [...loadedFiles].sort((a, b) => {
        if (a.scope === b.scope) return 0;
        return a.scope === "user" ? -1 : 1;
    });
    const userRaw = mergeFiles.find((f) => f.scope === "user")?.config;
    // The threshold trust boundary uses the effective USER/default config as its baseline.
    const trustedBaseConfig = parsePiConfig(userRaw ?? {}).config;
    let userTierFallback: Map<string, unknown> | undefined;

    for (const loaded of mergeFiles) {
        const prefix = loaded.scope === "user" ? "[user config]" : "[project config]";
        warnings.push(...loaded.warnings.map((warning) => `${prefix} ${warning}`));
        warnings.push(
            ...removedKeyWarnings(loaded.config).map((warning) => `${prefix} ${warning}`),
        );

        if (loaded.scope === "project") {
            // The loader sanitizes the untrusted project config before merging it.
            const projectRaw = { ...loaded.config };
            for (const warning of stripUnsafeProjectConfigFields(projectRaw)) {
                warnings.push(`${prefix} ${warning}`);
            }
            userTierFallback = userTierFallbackFor(projectRaw, userRaw, trustedBaseConfig);
            rawConfig = mergeRawConfigs(rawConfig, projectRaw);
            for (const warning of constrainProjectThresholdOverrides({
                mergedRaw: rawConfig,
                projectRaw,
                trustedBaseConfig,
            })) {
                warnings.push(`${prefix} ${warning}`);
            }
        } else {
            rawConfig = mergeRawConfigs(rawConfig, loaded.config);
        }
    }

    const parsed = parsePiConfig(rawConfig, { userTierFallback });
    setOutputReserveConfig(parsed.config.output_reserve);
    setWindowOverlayPath(parsed.config.models?.window_overlay_path);
    warnings.push(...parsed.warnings.map((warning) => `[merged config] ${warning}`));

    return {
        config: parsed.config,
        registrationPromptSurface: trustedBaseConfig.prompt_surface,
        warnings,
        loadedFromPaths: loadedFiles.map((loaded) => loaded.path),
    };
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
    loaded: LoadedConfigFile,
): Array<{ keyPath: string; source: "user" | "project"; message: string }> {
    if (loaded.warnings.length === 0 || loaded.loadOutcome !== "substitution-failure") {
        return [];
    }
    const emptyPaths = collectEmptyStringPaths(loaded.config);
    return loaded.warnings.map((message) => {
        const matchedPath = emptyPaths.find((path) => {
            const tail = path.split(".").at(-1) ?? path;
            return message.includes(path) || message.toLowerCase().includes(tail.toLowerCase());
        });
        return {
            keyPath: matchedPath ?? "<unknown>",
            source: loaded.scope,
            message,
        };
    });
}

function combinedOutcome(args: {
    sources: LoadPiConfigResultDetailed["sources"];
    substitutionFailures: LoadPiConfigResultDetailed["substitutionFailures"];
    recoveredTopLevelKeys: string[];
}): LoadOutcome {
    const sourceOutcomes = Object.values(args.sources);
    if (sourceOutcomes.includes("project-file-parse-error")) return "project-file-parse-error";
    if (sourceOutcomes.includes("project-file-io-error")) return "project-file-io-error";
    if (args.recoveredTopLevelKeys.length > 0 || sourceOutcomes.includes("schema-recovery")) {
        return "schema-recovery";
    }
    if (args.substitutionFailures.length > 0) return "substitution-failure";
    return "ok";
}

export function loadPiConfigDetailed(opts: LoadPiConfigOptions = {}): LoadPiConfigResultDetailed {
    const cwd = opts.cwd ?? process.cwd();
    const loadedFiles: LoadedConfigFile[] = [];
    const warnings: string[] = [];

    const projectPath = resolveFirstExisting(getProjectConfigPaths(cwd));
    if (projectPath) {
        const loaded = loadConfigFile(projectPath, "project");
        if (loaded) loadedFiles.push(loaded);
    }

    const userPath = resolveFirstExisting(getUserConfigPaths());
    if (userPath) {
        const loaded = loadConfigFile(userPath, "user");
        if (loaded) loadedFiles.push(loaded);
    }

    let rawConfig: Record<string, unknown> = {};
    const mergeFiles = [...loadedFiles].sort((a, b) => {
        if (a.scope === b.scope) return 0;
        return a.scope === "user" ? -1 : 1;
    });
    const userRaw = mergeFiles.find((f) => f.scope === "user")?.config;
    // A cloned repository may delay compaction but must not lower thresholds enough to increase historian work for the user's account.
    const trustedBaseConfig = parsePiConfig(userRaw ?? {}).config;
    let userTierFallback: Map<string, unknown> | undefined;

    for (const loaded of mergeFiles) {
        const prefix = loaded.scope === "user" ? "[user config]" : "[project config]";
        warnings.push(...loaded.warnings.map((warning) => `${prefix} ${warning}`));
        warnings.push(
            ...removedKeyWarnings(loaded.config).map((warning) => `${prefix} ${warning}`),
        );

        if (loaded.scope === "project") {
            const projectRaw = { ...loaded.config };
            for (const warning of stripUnsafeProjectConfigFields(projectRaw)) {
                warnings.push(`${prefix} ${warning}`);
            }
            userTierFallback = userTierFallbackFor(projectRaw, userRaw, trustedBaseConfig);
            rawConfig = mergeRawConfigs(rawConfig, projectRaw);
            for (const warning of constrainProjectThresholdOverrides({
                mergedRaw: rawConfig,
                projectRaw,
                trustedBaseConfig,
            })) {
                warnings.push(`${prefix} ${warning}`);
            }
        } else {
            rawConfig = mergeRawConfigs(rawConfig, loaded.config);
        }
    }

    const recoveredTopLevelKeys: string[] = [];
    const parsed = parsePiConfig(rawConfig, { recoveredTopLevelKeys, userTierFallback });
    setOutputReserveConfig(parsed.config.output_reserve);
    setWindowOverlayPath(parsed.config.models?.window_overlay_path);
    warnings.push(...parsed.warnings.map((warning) => `[merged config] ${warning}`));
    const substitutionFailures = loadedFiles.flatMap(bindSubstitutionFailures);
    const userLoaded = loadedFiles.find((loaded) => loaded.scope === "user");
    const projectLoaded = loadedFiles.find((loaded) => loaded.scope === "project");
    const sources = {
        userConfig: userLoaded?.loadOutcome ?? ("ok" as LoadOutcome),
        projectConfig: projectLoaded?.loadOutcome ?? ("ok" as LoadOutcome),
    };

    return {
        config: parsed.config,
        registrationPromptSurface: trustedBaseConfig.prompt_surface,
        warnings,
        loadedFromPaths: loadedFiles.map((loaded) => loaded.path),
        loadOutcome: combinedOutcome({
            sources,
            substitutionFailures,
            recoveredTopLevelKeys,
        }),
        sources,
        substitutionFailures,
        recoveredTopLevelKeys,
    };
}
