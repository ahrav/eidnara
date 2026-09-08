import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { basename, dirname } from "node:path";
import { resolveEidnaraProjectConfigPath } from "@eidnara/opencode/config/config-paths";
import {
    type ConflictResult,
    DCP_CONFLICT_REASON,
    detectConflicts,
    hasOmoPlugin,
    openCodeConfigLayerPaths,
    pluginEntriesOutside,
    projectOpenCodeConfigPaths,
} from "@eidnara/opencode/shared/conflict-detector";
import { collectOmoConfigPaths, fixConflicts } from "@eidnara/opencode/shared/conflict-fixer";
import {
    appendJsoncArrayValues,
    removeJsoncArrayEntries,
    setJsoncValue,
} from "@eidnara/opencode/shared/jsonc-edit";
import { isRecord } from "@eidnara/opencode/shared/record-type-guard";
import { stringify as stringifyJsonc } from "comment-json";
import {
    isDevPathPluginEntry,
    isLocalPathPluginEntry,
    matchesPluginEntry,
} from "../adapters/opencode";
import { type AgentBlockKind, pruneInvalidAgentFields } from "../lib/agent-config";
import { writeFileAtomic } from "../lib/atomic-write";
import { type EidnaraModes, projectModeOverrides, readEidnaraModes } from "../lib/eidnara-modes";
import { restoreFiles, snapshotFiles } from "../lib/file-snapshot";
import {
    assertJsoncConfigsParseable,
    readJsoncConfigForUpdate,
    readJsoncLenient,
} from "../lib/jsonc-config";
import { pickModel } from "../lib/model-picker";
import { detectOpenCode } from "../lib/opencode-detect";
import { getAvailableModels, getOpenCodeVersion } from "../lib/opencode-helpers";
import { type ConfigPaths, detectConfigPaths } from "../lib/paths";
import { confirm, intro, log, note, outro, promptIO, spinner } from "../lib/prompts";
import { compareVersionStrings } from "../lib/version";

const PLUGIN_NAME = "@eidnara/opencode";
const DCP_PLUGIN_NAME = "@tarquinen/opencode-dcp";
/** Mirrors the `@opencode-ai/plugin` peer dependency in `packages/opencode-plugin/package.json`. */
export const OPENCODE_MINIMUM_VERSION = "1.15.0";

/** With `enabled: false` the plugin skips every hook at startup, so native compaction must stay on. commentlint: allow(JUDGE) */
function resolveWriterModes(sharedConfigPath: string, directory: string): EidnaraModes {
    const modes = readEidnaraModes(sharedConfigPath);
    if (!modes.enabled) {
        log.warn(
            `Eidnara is disabled (\`enabled: false\`) in ${sharedConfigPath}; setup keeps that setting and leaves OpenCode's native compaction on.`,
        );
    }
    const projectConfigPath = resolveEidnaraProjectConfigPath(directory);
    const overrides = projectModeOverrides(projectConfigPath, modes);
    if (overrides.length > 0) {
        log.warn(
            `Project config ${projectConfigPath} overrides ${overrides.join(", ")}; OpenCode's native settings follow the shared config, so this project may run both Eidnara and native compaction, or neither. Adjust one of the configs if that is not intended.`,
        );
    }
    return modes;
}

function ensureDir(dir: string): void {
    if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
    }
}

export function addPluginToOpenCodeConfig(
    configPath: string,
    _format: "json" | "jsonc" | "none",
    removeDcp = false,
    /**
     * When compactionEnabled is false, the writer does not write compaction.auto or compaction.prune.
     * When compactionEnabled is false, the writer does not change compaction fields.
     */
    compactionEnabled = true,
    /** Plugin entries already effective from other config layers, such as a project config's dev path; an Eidnara entry among them suppresses the global one so OpenCode does not load the plugin twice. commentlint: allow(JUDGE) */
    effectiveElsewhere: readonly unknown[] = [],
): void {
    const existsAtCommit = existsSync(configPath);
    const existing = existsAtCommit ? readJsoncConfigForUpdate(configPath) : {};
    assertPluginListValue(configPath, existing.plugin);
    const registeredElsewhere = effectiveElsewhere.some(
        (plugin) => matchesPluginEntry(plugin, PLUGIN_NAME) || isDevPathPluginEntry(plugin),
    );
    if (registeredElsewhere) {
        log.info(
            "Eidnara is already registered by a project OpenCode config; not adding it globally.",
        );
    }
    if (!existsAtCommit) {
        ensureDir(dirname(configPath));
        const created: Record<string, unknown> = registeredElsewhere
            ? {}
            : { plugin: [PLUGIN_NAME] };
        if (compactionEnabled) {
            created.compaction = { auto: false, prune: false };
        }
        writeFileAtomic(configPath, `${stringifyJsonc(created, null, 2)}\n`);
        return;
    }

    let text = readFileSync(configPath, "utf-8");
    let changed = false;
    const rawPlugins: unknown[] = Array.isArray(existing.plugin) ? existing.plugin : [];
    const retainedPlugins = removeDcp
        ? rawPlugins.filter((plugin) => !matchesPluginEntry(plugin, DCP_PLUGIN_NAME))
        : rawPlugins;

    if (
        retainedPlugins.some(
            (plugin) =>
                isLocalPathPluginEntry(plugin) &&
                String(plugin).includes("context") &&
                !isDevPathPluginEntry(plugin),
        )
    ) {
        log.warn(
            "An unverifiable local OpenCode plugin path was ignored; its package name is not Eidnara.",
        );
    }

    if (Array.isArray(existing.plugin)) {
        if (removeDcp) {
            const result = removeJsoncArrayEntries(text, ["plugin"], (plugin) =>
                matchesPluginEntry(plugin, DCP_PLUGIN_NAME),
            );
            if (result.removed) {
                text = result.text;
                changed = true;
            }
        }

        const hasNpmEntry = retainedPlugins.some((plugin) =>
            matchesPluginEntry(plugin, PLUGIN_NAME),
        );
        const hasDevEntry = retainedPlugins.some((plugin) => isDevPathPluginEntry(plugin));
        if (!hasNpmEntry && !hasDevEntry && !registeredElsewhere) {
            text = appendJsoncArrayValues(text, ["plugin"], [PLUGIN_NAME]);
            changed = true;
        }
    } else if (!registeredElsewhere) {
        text = setJsoncValue(text, ["plugin"], [PLUGIN_NAME]);
        changed = true;
    }

    // When compactionEnabled is false, the writer does not change compaction fields.
    // When compactionEnabled is true, the writer sets compaction.auto and compaction.prune to false.
    if (compactionEnabled) {
        const compaction = existing.compaction;
        const hasCompactionObject =
            typeof compaction === "object" && compaction !== null && !Array.isArray(compaction);
        if (!hasCompactionObject) {
            text = setJsoncValue(text, ["compaction"], { auto: false, prune: false });
            changed = true;
        } else {
            const fields = compaction as Record<string, unknown>;
            if (fields.auto !== false) {
                text = setJsoncValue(text, ["compaction", "auto"], false);
                changed = true;
            }
            if (fields.prune !== false) {
                text = setJsoncValue(text, ["compaction", "prune"], false);
                changed = true;
            }
        }
    }

    if (changed) writeFileAtomic(configPath, text);
}

export function addPluginToTuiConfig(configPath: string, _format: "json" | "jsonc" | "none"): void {
    const existsAtCommit = existsSync(configPath);
    const existing = existsAtCommit ? readJsoncConfigForUpdate(configPath) : {};
    assertPluginListValue(configPath, existing.plugin);
    if (!existsAtCommit) {
        ensureDir(dirname(configPath));
        writeFileAtomic(configPath, `${stringifyJsonc({ plugin: [PLUGIN_NAME] }, null, 2)}\n`);
        return;
    }

    const rawPlugins: unknown[] = Array.isArray(existing.plugin) ? existing.plugin : [];
    if (
        rawPlugins.some(
            (plugin) =>
                isLocalPathPluginEntry(plugin) &&
                String(plugin).includes("context") &&
                !isDevPathPluginEntry(plugin),
        )
    ) {
        log.warn(
            "An unverifiable local TUI plugin path was ignored; its package name is not Eidnara.",
        );
    }

    const hasNpmEntry = rawPlugins.some((plugin) => matchesPluginEntry(plugin, PLUGIN_NAME));
    const hasDevEntry = rawPlugins.some((plugin) => isDevPathPluginEntry(plugin));
    if (hasNpmEntry || hasDevEntry) return;

    const text = Array.isArray(existing.plugin)
        ? appendJsoncArrayValues(readFileSync(configPath, "utf-8"), ["plugin"], [PLUGIN_NAME])
        : setJsoncValue(readFileSync(configPath, "utf-8"), ["plugin"], [PLUGIN_NAME]);
    writeFileAtomic(configPath, text);
}

export function findDcpPluginIndexes(plugins: unknown[]): number[] {
    return plugins
        .map((plugin, index) => (matchesPluginEntry(plugin, DCP_PLUGIN_NAME) ? index : -1))
        .filter((index) => index >= 0);
}

function pluginEntryName(entry: unknown): string {
    if (typeof entry === "string") return entry;
    if (Array.isArray(entry) && typeof entry[0] === "string") return entry[0];
    return String(entry);
}

/** `keep` records an explicit refusal, which the broader conflict-fix pass must honor. */
type DcpDecision = "absent" | "remove" | "keep";

async function resolveDcpConflictBeforeSetup(
    configPath: string,
    format: "json" | "jsonc" | "none",
): Promise<DcpDecision> {
    if (format === "none") return "absent";
    const ocConfig = readJsoncConfigForUpdate(configPath);
    const plugins = Array.isArray(ocConfig.plugin) ? ocConfig.plugin : [];
    const dcpIndexes = findDcpPluginIndexes(plugins);
    if (dcpIndexes.length === 0) return "absent";

    log.warn(`Found conflicting plugin: ${pluginEntryName(plugins[dcpIndexes[0]])}`);
    log.message(
        "opencode-dcp (Dynamic Context Pruning) and Eidnara both manage context.\n" +
            "Running both simultaneously will cause unpredictable behavior.",
    );
    const shouldRemove = await confirm("Remove opencode-dcp from your config?", true);
    if (!shouldRemove) {
        log.warn("Skipped — you may experience context management conflicts");
    }
    return shouldRemove ? "remove" : "keep";
}

/**
 * Drop the DCP conflict from a detection result so a later "apply automatic
 * fixes" answer cannot remove a plugin the user chose to keep.
 */
export function withoutDcpConflict(result: ConflictResult): ConflictResult {
    const reasons = result.reasons.filter((reason) => reason !== DCP_CONFLICT_REASON);
    return {
        ...result,
        hasConflict: reasons.length > 0,
        reasons,
        conflicts: { ...result.conflicts, dcpPlugin: false },
    };
}

export function writeEidnaraConfig(
    configPath: string,
    options: {
        historianModel: string | null;
        sidekickEnabled: boolean;
        sidekickModel: string | null;
        claudeMax: boolean;
    },
): void {
    const config = readJsoncConfigForUpdate(configPath);

    if (!config.$schema) {
        config.$schema =
            "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json";
    }

    if (options.historianModel) {
        const historian = asPlainRecord(config.historian);
        historian.model = options.historianModel;
        delete historian.disable;
        delete historian.enabled;
        warnPrunedAgentFields(
            configPath,
            "historian",
            pruneInvalidAgentFields("historian", historian),
        );
        config.historian = historian;
    }

    const sidekick = asPlainRecord(config.sidekick);
    delete sidekick.enabled;
    if (options.sidekickEnabled) {
        delete sidekick.disable;
        if (options.sidekickModel) {
            sidekick.model = options.sidekickModel;
        }
    } else {
        sidekick.disable = true;
    }
    warnPrunedAgentFields(configPath, "sidekick", pruneInvalidAgentFields("sidekick", sidekick));
    config.sidekick = sidekick;

    if (options.claudeMax) {
        config.cache_ttl = withClaudeMaxCacheTtl(config.cache_ttl, [
            options.historianModel,
            options.sidekickModel,
        ]);
    }

    writeFileAtomic(configPath, `${stringifyJsonc(config, null, 2)}\n`);
}

/**
 * A parseable config can still hold a schema-invalid block such as `"historian": "old-model"`; config loading logs "invalid agent configuration, ignoring" for it, and the writer starts fresh the same way. commentlint: allow(JUDGE)
 * A plain object is returned as is because comment-json keeps a block's comments as symbol-keyed metadata that a spread copy loses. commentlint: allow(JUDGE)
 */
function asPlainRecord(value: unknown): Record<string, unknown> {
    return isRecord(value) ? value : {};
}

/**
 * Normalize a scalar `cache_ttl` into `{ default: existing }` before adding
 * per-model overrides. Selected Anthropic models receive the same 59m TTL as
 * the fixed overrides.
 */
export function withClaudeMaxCacheTtl(
    existing: unknown,
    selectedModels: readonly (string | null)[] = [],
): Record<string, string> {
    // The schema types every `cache_ttl` value as a string; a non-string value would fail the whole record.
    // An existing record is pruned in place so comment-json's comment metadata survives.
    const record = typeof existing === "string" ? { default: existing } : asPlainRecord(existing);
    for (const [key, value] of Object.entries(record)) {
        if (typeof value !== "string" || value.length === 0) delete record[key];
    }
    const cacheTtl = record as Record<string, string>;
    if (!cacheTtl.default) cacheTtl.default = "5m";
    cacheTtl["anthropic/claude-sonnet-4-6"] = "59m";
    cacheTtl["anthropic/claude-opus-4-6"] = "59m";
    for (const model of selectedModels) {
        if (model?.startsWith("anthropic/")) cacheTtl[model] = "59m";
    }
    return cacheTtl;
}

function warnPrunedAgentFields(configPath: string, kind: AgentBlockKind, removed: string[]): void {
    if (removed.length === 0) return;
    log.warn(
        `Dropped invalid ${kind} field${removed.length > 1 ? "s" : ""} ${removed.join(", ")} from ${configPath}; the plugin would otherwise ignore the whole ${kind} block.`,
    );
}

/** Chosen models count too: `pickModel` accepts manual entry when discovery returns nothing. */
export function hasAnthropicModel(models: readonly (string | null)[]): boolean {
    return models.some((model) => model?.startsWith("anthropic/") ?? false);
}

/**
 * `detectConflicts` and `fixConflicts` skip unparseable files, so these repair targets are checked before any write. commentlint: allow(JUDGE)
 * Only the effective member of each project `.jsonc`/`.json` pair is listed,
 * matching the file OpenCode loads, so a stale shadowed sibling cannot block setup.
 * OMO files count only when a conflict repair can reach them; the caller
 * decides that from the enabled mode, the OMO plugin entry, and the
 * first-time branch.
 */
export function preflightConfigPaths(
    paths: ConfigPaths & { eidnaraConfig: string },
    directory: string,
    options: { omoRepairReachable: boolean },
): string[] {
    const [dotOcJsonc, dotOcJson, rootJsonc, rootJson] = projectOpenCodeConfigPaths(directory);
    return [
        paths.opencodeConfig,
        paths.eidnaraConfig,
        paths.tuiConfig,
        existsSync(dotOcJsonc) ? dotOcJsonc : dotOcJson,
        existsSync(rootJsonc) ? rootJsonc : rootJson,
        ...(options.omoRepairReachable ? collectOmoConfigPaths(directory) : []),
    ];
}

/**
 * The plugin writers append to an array and would replace any other `plugin`
 * value wholesale, so a hand-written scalar or object entry is refused up
 * front rather than silently discarded.
 */
export function assertPluginListShape(configPaths: readonly string[]): void {
    for (const configPath of configPaths) {
        assertPluginListValue(configPath, readJsoncLenient(configPath).value.plugin);
    }
}

/** The writers repeat this check on the value they re-read at commit time, since a file can change while prompts are open. */
function assertPluginListValue(configPath: string, plugin: unknown): void {
    if (plugin !== undefined && !Array.isArray(plugin)) {
        throw new Error(
            `Refusing to overwrite ${configPath}: "plugin" must be an array of plugin entries, found ${JSON.stringify(plugin)}`,
        );
    }
}

export async function runSetup(dryRun = false): Promise<number> {
    intro("Eidnara — Setup");
    if (dryRun) {
        log.warn("Dry run — no files will be written and no config will be changed.");
    }

    const s = spinner();
    s.start("Checking OpenCode installation");

    const detection = detectOpenCode();
    if (detection.kind === "none") {
        s.stop("OpenCode not found");
        const shouldContinue = await confirm(
            "OpenCode not found on PATH. Continue setup anyway?",
            false,
        );
        if (!shouldContinue) {
            log.info("Install OpenCode: https://opencode.ai");
            outro("Setup cancelled");
            return 1;
        }
    } else if (detection.kind === "desktop") {
        // absent.
        s.stop("OpenCode Desktop detected (CLI not installed)");
        log.info(
            "Model auto-discovery needs the OpenCode CLI; you will enter models manually. Install the CLI to auto-populate: https://opencode.ai",
        );
    } else {
        const version = getOpenCodeVersion(detection.binary);
        s.stop(`OpenCode ${version ?? ""} detected`);
        if (version && compareVersionStrings(version, OPENCODE_MINIMUM_VERSION) < 0) {
            log.warn(
                `OpenCode ${version} is older than the required ${OPENCODE_MINIMUM_VERSION}; the plugin may fail to load.`,
            );
            const proceed = await confirm("Continue with setup anyway?", false);
            if (!proceed) {
                outro("Setup cancelled — upgrade OpenCode and try again.");
                return 1;
            }
        }
    }

    s.start("Fetching available models");

    const allModels = detection.kind === "cli" ? getAvailableModels(detection.binary) : [];
    if (allModels.length > 0) {
        s.stop(`Found ${allModels.length} models`);
    } else {
        s.stop("No models found");
        log.warn("You can configure models manually in eidnara.jsonc later");
    }

    const detected = detectConfigPaths();
    if (detected.eidnaraConfig === undefined) {
        log.error(
            "No user configuration directory: set HOME (or XDG_CONFIG_HOME) to an absolute path so eidnara.jsonc has a location.",
        );
        outro("Setup stopped.");
        return 1;
    }
    // The guard above narrows the user config path for every write and preflight that follows.
    const paths = { ...detected, eidnaraConfig: detected.eidnaraConfig };
    // Shared Eidnara config can come from Pi or OMP; only OpenCode config files establish an OpenCode setup. commentlint: allow(JUDGE)
    // Project-level OpenCode configs are included because `detectConflicts` and `fixConflicts` read and repair them.
    const hadExistingSetup =
        paths.opencodeConfigFormat !== "none" ||
        paths.tuiConfigFormat !== "none" ||
        projectOpenCodeConfigPaths(process.cwd()).some((path) => existsSync(path));
    // With Eidnara disabled nothing conflicts, so no conflict repair is offered in that mode.
    const modes = resolveWriterModes(paths.eidnaraConfig, process.cwd());
    const compactionEnabled = modes.compactionEnabled;
    const omoConfigs = collectOmoConfigPaths(process.cwd());
    const firstTimeOmoRepair = modes.enabled && omoConfigs.length > 0 && !hadExistingSetup;
    const omoRepairReachable = modes.enabled && (hasOmoPlugin(process.cwd()) || firstTimeOmoRepair);

    // The preflight is read-only, so a dry run performs it too and predicts the refusal a real run would make.
    try {
        assertJsoncConfigsParseable(
            preflightConfigPaths(paths, process.cwd(), { omoRepairReachable }),
        );
        assertPluginListShape([paths.opencodeConfig, paths.tuiConfig]);
    } catch (error) {
        log.error(error instanceof Error ? error.message : String(error));
        outro("Setup stopped — fix the malformed config and rerun setup.");
        return 1;
    }

    const dcpDecision: DcpDecision =
        dryRun || !modes.enabled
            ? "absent"
            : await resolveDcpConflictBeforeSetup(paths.opencodeConfig, paths.opencodeConfigFormat);
    const removeDcp = dcpDecision === "remove";

    if (dryRun) {
        log.message(
            compactionEnabled
                ? `[dry-run] would add the plugin to ${paths.opencodeConfig} and disable compaction`
                : `[dry-run] would add the plugin to ${paths.opencodeConfig} (compaction-off mode — native compaction fields left untouched)`,
        );
    }

    let conflictFix: Parameters<typeof fixConflicts>[1] | null = null;
    // A declined fix covers the native compaction flags too; the writer must not apply them anyway.
    let keepNativeCompaction = false;
    if (hadExistingSetup && modes.enabled) {
        const detected = detectConflicts(process.cwd(), {
            compactionEnabled,
        });
        const conflicts = dcpDecision === "keep" ? withoutDcpConflict(detected) : detected;
        if (conflicts.hasConflict) {
            log.warn("Found conflicting configuration that can disable Eidnara:");
            for (const reason of conflicts.reasons) {
                log.message(`  • ${reason}`);
            }

            if (dryRun) {
                log.message("[dry-run] would offer to apply automatic conflict fixes");
            } else {
                const shouldFixConflicts = await confirm(
                    "Apply automatic conflict fixes to your OpenCode and OMO config files?",
                    true,
                );

                if (shouldFixConflicts) {
                    conflictFix = conflicts.conflicts;
                } else {
                    keepNativeCompaction =
                        conflicts.conflicts.compactionAuto || conflicts.conflicts.compactionPrune;
                    log.warn("Skipped automatic conflict fixes — Eidnara may remain disabled");
                }
            }
        }
    }

    const historianModel = await pickModel(promptIO, allModels, "historian");
    log.success(`Historian: ${historianModel}`);

    const sidekickEnabled = await confirm("Enable sidekick?", false);
    let sidekickModel: string | null = null;
    if (sidekickEnabled) {
        sidekickModel = await pickModel(promptIO, allModels, "sidekick");
        log.success(`Sidekick: ${sidekickModel}`);
    }

    const hasAnthropic = hasAnthropicModel([...allModels, historianModel, sidekickModel]);
    let claudeMax = false;
    if (hasAnthropic) {
        log.message(
            "Claude Max/Pro subscribers get extended prompt caching (up to 1 hour).\n" +
                "This lets Eidnara defer context operations much longer, saving money.",
        );
        claudeMax = await confirm("Do you have a Claude Max or Pro subscription?", false);
        if (claudeMax) {
            log.success("Cache TTL set to 59m for Anthropic models");
        }
    }

    if (dryRun) {
        log.message(`[dry-run] would write Eidnara config to ${paths.eidnaraConfig}`);
        log.message(`[dry-run] would add the TUI sidebar plugin to ${paths.tuiConfig}`);
    }

    // Existing users receive the OMO hook fixes in the `hadExistingSetup` conflict pass above.
    let disableOmoHooks = false;
    if (firstTimeOmoRepair) {
        log.warn(`Found oh-my-opencode config: ${omoConfigs.join(", ")}`);
        log.message(
            "These hooks may conflict:\n" +
                "  • context-window-monitor\n" +
                "  • preemptive-compaction\n" +
                "  • anthropic-context-window-limit-recovery",
        );

        if (dryRun) {
            log.message("[dry-run] would offer to disable conflicting oh-my-opencode hooks");
        } else if (await confirm("Disable these hooks in oh-my-opencode?", true)) {
            disableOmoHooks = true;
        } else {
            log.warn("Skipped — you may experience context management conflicts");
        }
    }

    const disableNativeCompaction = compactionEnabled && !keepNativeCompaction;
    let repairIncomplete = false;
    if (!dryRun) {
        // Every file a later step may write is captured first, so a failure part-way (a read-only
        // directory, for example) restores the OpenCode registration and compaction flags instead
        // of leaving the plugin active without its config.
        const snapshot = snapshotFiles([
            ...preflightConfigPaths(paths, process.cwd(), { omoRepairReachable }),
            ...openCodeConfigLayerPaths(process.cwd()),
        ]);
        try {
            addPluginToOpenCodeConfig(
                paths.opencodeConfig,
                paths.opencodeConfigFormat,
                removeDcp,
                disableNativeCompaction,
                pluginEntriesOutside(process.cwd(), paths.opencodeConfig),
            );
            log.success(`Plugin added to ${paths.opencodeConfig}`);
            if (removeDcp) log.success("Removed opencode-dcp from plugin list");
            if (disableNativeCompaction) {
                log.info("Disabled built-in compaction (auto=false, prune=false)");
                log.message(
                    "Eidnara handles context management — built-in compaction would interfere",
                );
            } else if (keepNativeCompaction) {
                log.warn(
                    "Left built-in compaction unchanged because automatic conflict fixes were declined — Eidnara stays disabled until compaction.auto and compaction.prune are false",
                );
            } else {
                log.info("Compaction-off mode active — leaving native compaction config untouched");
            }

            if (conflictFix) {
                const actions = fixConflicts(process.cwd(), conflictFix, {
                    compactionEnabled,
                });
                if (actions.length > 0) {
                    for (const action of actions) log.success(action);
                } else {
                    log.info("No additional conflict changes were needed");
                }
                // The fixer edits only files that exist, so an accepted repair can leave a conflict in place
                // (an OMO plugin entry with no OMO config file, for example); re-detect and say so.
                const remaining = detectConflicts(process.cwd(), { compactionEnabled });
                if (remaining.hasConflict) {
                    repairIncomplete = true;
                    log.warn(
                        "Conflicts remain after the automatic fixes; Eidnara stays disabled until they are resolved:",
                    );
                    for (const reason of remaining.reasons) log.message(`  • ${reason}`);
                    log.message(
                        "For oh-my-opencode without a config file, add `disabled_hooks` (context-window-monitor, preemptive-compaction, anthropic-context-window-limit-recovery) to its config, then rerun setup.",
                    );
                }
            }

            writeEidnaraConfig(paths.eidnaraConfig, {
                historianModel,
                sidekickEnabled,
                sidekickModel,
                claudeMax,
            });
            log.success(`Config written to ${paths.eidnaraConfig}`);
            addPluginToTuiConfig(paths.tuiConfig, paths.tuiConfigFormat);
            log.success(`TUI sidebar plugin added to ${basename(paths.tuiConfig)}`);

            if (disableOmoHooks) {
                const actions = fixConflicts(
                    process.cwd(),
                    {
                        compactionAuto: false,
                        compactionPrune: false,
                        dcpPlugin: false,
                        omoPreemptiveCompaction: true,
                        omoContextWindowMonitor: true,
                        omoAnthropicRecovery: true,
                    },
                    {
                        compactionEnabled,
                    },
                );
                if (actions.includes("Disabled conflicting oh-my-opencode hooks")) {
                    log.success("Hooks disabled in oh-my-opencode config");
                }
            }
        } catch (error) {
            log.error(error instanceof Error ? error.message : String(error));
            try {
                restoreFiles(snapshot);
            } catch (rollbackError) {
                log.error(
                    rollbackError instanceof Error ? rollbackError.message : String(rollbackError),
                );
                outro(
                    "Setup stopped — changes were only partly rolled back; restore the files above by hand.",
                );
                return 1;
            }
            outro("Setup stopped — rolled back OpenCode changes.");
            return 1;
        }
    }

    const summary = [
        `Plugin: ${PLUGIN_NAME}`,
        disableNativeCompaction
            ? "Compaction: disabled (Eidnara manages the window)"
            : keepNativeCompaction
              ? "Compaction: built-in compaction left on (conflict fixes declined)"
              : modes.enabled
                ? "Compaction: native settings left unchanged (Eidnara compaction is off)"
                : "Compaction: native settings left unchanged (Eidnara is disabled)",
        historianModel ? `Historian: ${historianModel}` : "Historian: fallback chain",
        sidekickEnabled
            ? `Sidekick: enabled${sidekickModel ? ` (${sidekickModel})` : ""}`
            : "Sidekick: disabled",
    ].join("\n");

    note(summary, dryRun ? "Configuration (dry run — not written)" : "Configuration");

    if (dryRun) {
        outro("Dry run complete — nothing was written.");
        return 0;
    }

    if (repairIncomplete) {
        outro(
            "Setup finished with warnings — resolve the remaining conflicts, then run 'opencode'.",
        );
        return 1;
    }
    outro("Run 'opencode' to start!");

    return 0;
}
