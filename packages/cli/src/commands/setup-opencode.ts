import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { basename, dirname } from "node:path";
import { loadPluginConfig } from "@eidnara/opencode/config";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import {
    type ConflictResult,
    DCP_CONFLICT_REASON,
    detectConflicts,
    projectOpenCodeConfigPaths,
} from "@eidnara/opencode/shared/conflict-detector";
import { collectOmoConfigPaths, fixConflicts } from "@eidnara/opencode/shared/conflict-fixer";
import {
    appendJsoncArrayValues,
    removeJsoncArrayEntries,
    setJsoncValue,
} from "@eidnara/opencode/shared/jsonc-edit";
import { stringify as stringifyJsonc } from "comment-json";
import {
    isDevPathPluginEntry,
    isLocalPathPluginEntry,
    matchesPluginEntry,
} from "../adapters/opencode";
import { writeFileAtomic } from "../lib/atomic-write";
import { assertJsoncConfigsParseable, readJsoncConfigForUpdate } from "../lib/jsonc-config";
import { pickModel } from "../lib/model-picker";
import { detectOpenCode } from "../lib/opencode-detect";
import { getAvailableModels, getOpenCodeVersion } from "../lib/opencode-helpers";
import { type ConfigPaths, detectConfigPaths } from "../lib/paths";
import { confirm, intro, log, note, outro, promptIO, spinner } from "../lib/prompts";

const PLUGIN_NAME = "@eidnara/opencode";
const DCP_PLUGIN_NAME = "@tarquinen/opencode-dcp";

/** With `enabled: false` the plugin skips every hook at startup, so native compaction must stay on. commentlint: allow(JUDGE) */
function resolveCompactionEnabledForWriter(): boolean {
    try {
        const config = loadPluginConfig(process.cwd());
        if (config.enabled === false) {
            log.warn(
                "Eidnara is disabled in its config (enabled: false); leaving native compaction untouched. " +
                    "Set enabled to true to let Eidnara manage the context window.",
            );
            return false;
        }
        return isCompactionEnabled(config);
    } catch (error) {
        log.warn(
            `Could not load Eidnara config to resolve compaction mode; ` +
                `preserving existing native compaction fields. ` +
                `(${error instanceof Error ? error.message : String(error)})`,
        );
        return false;
    }
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
): void {
    const existsAtCommit = existsSync(configPath);
    const existing = existsAtCommit ? readJsoncConfigForUpdate(configPath) : {};
    if (!existsAtCommit) {
        ensureDir(dirname(configPath));
        const created: Record<string, unknown> = { plugin: [PLUGIN_NAME] };
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
        if (!hasNpmEntry && !hasDevEntry) {
            text = appendJsoncArrayValues(text, ["plugin"], [PLUGIN_NAME]);
            changed = true;
        }
    } else {
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
        const historian = (config.historian as Record<string, unknown>) ?? {};
        historian.model = options.historianModel;
        config.historian = historian;
    }

    const sidekick = (config.sidekick as Record<string, unknown>) ?? {};
    delete sidekick.enabled;
    if (options.sidekickEnabled) {
        delete sidekick.disable;
        if (options.sidekickModel) {
            sidekick.model = options.sidekickModel;
        }
        config.sidekick = sidekick;
    } else {
        sidekick.disable = true;
        config.sidekick = sidekick;
    }

    if (options.claudeMax) {
        config.cache_ttl = withClaudeMaxCacheTtl(config.cache_ttl, [
            options.historianModel,
            options.sidekickModel,
        ]);
    }

    writeFileAtomic(configPath, `${stringifyJsonc(config, null, 2)}\n`);
}

/**
 * Normalize a scalar `cache_ttl` into `{ default: existing }` before adding
 * per-model overrides. Setting a key on a string primitive throws under strict mode.
 * Selected Anthropic models receive the same 59m TTL as the fixed overrides.
 */
export function withClaudeMaxCacheTtl(
    existing: unknown,
    selectedModels: readonly (string | null)[] = [],
): Record<string, string> {
    const cacheTtl: Record<string, string> =
        typeof existing === "string"
            ? { default: existing }
            : typeof existing === "object" && existing !== null && !Array.isArray(existing)
              ? { ...(existing as Record<string, string>) }
              : {};
    if (!cacheTtl.default) cacheTtl.default = "5m";
    cacheTtl["anthropic/claude-sonnet-4-6"] = "59m";
    cacheTtl["anthropic/claude-opus-4-6"] = "59m";
    for (const model of selectedModels) {
        if (model?.startsWith("anthropic/")) cacheTtl[model] = "59m";
    }
    return cacheTtl;
}

/** Chosen models count too: `pickModel` accepts manual entry when discovery returns nothing. */
export function hasAnthropicModel(models: readonly (string | null)[]): boolean {
    return models.some((model) => model?.startsWith("anthropic/") ?? false);
}

/**
 * `detectConflicts` and `fixConflicts` skip unparseable files, so these repair targets are checked before any write. commentlint: allow(JUDGE)
 * Only the effective member of each project `.jsonc`/`.json` pair is listed,
 * matching the file OpenCode loads, so a stale shadowed sibling cannot block setup.
 */
export function preflightConfigPaths(paths: ConfigPaths, directory: string): string[] {
    const [dotOcJsonc, dotOcJson, rootJsonc, rootJson] = projectOpenCodeConfigPaths(directory);
    return [
        paths.opencodeConfig,
        paths.eidnaraConfig,
        paths.tuiConfig,
        existsSync(dotOcJsonc) ? dotOcJsonc : dotOcJson,
        existsSync(rootJsonc) ? rootJsonc : rootJson,
        ...collectOmoConfigPaths(directory),
    ];
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
    }

    s.start("Fetching available models");

    const allModels = detection.kind === "cli" ? getAvailableModels(detection.binary) : [];
    if (allModels.length > 0) {
        s.stop(`Found ${allModels.length} models`);
    } else {
        s.stop("No models found");
        log.warn("You can configure models manually in eidnara.jsonc later");
    }

    const paths = detectConfigPaths();
    // A project-level OpenCode config counts: `detectConflicts` and
    // `fixConflicts` read and repair those files, so a first-time user running
    // setup inside such a project must not skip the conflict pass.
    const hadExistingSetup =
        paths.opencodeConfigFormat !== "none" ||
        existsSync(paths.eidnaraConfig) ||
        paths.tuiConfigFormat !== "none" ||
        projectOpenCodeConfigPaths(process.cwd()).some((path) => existsSync(path));

    if (!dryRun) {
        try {
            assertJsoncConfigsParseable(preflightConfigPaths(paths, process.cwd()));
        } catch (error) {
            log.error(error instanceof Error ? error.message : String(error));
            outro("Setup stopped — fix the malformed config and rerun setup.");
            return 1;
        }
    }

    const dcpDecision: DcpDecision = dryRun
        ? "absent"
        : await resolveDcpConflictBeforeSetup(paths.opencodeConfig, paths.opencodeConfigFormat);
    const removeDcp = dcpDecision === "remove";

    const compactionEnabled = resolveCompactionEnabledForWriter();

    if (dryRun) {
        log.message(
            compactionEnabled
                ? `[dry-run] would add the plugin to ${paths.opencodeConfig} and disable compaction`
                : `[dry-run] would add the plugin to ${paths.opencodeConfig} (compaction-off mode — native compaction fields left untouched)`,
        );
    }

    let conflictFix: Parameters<typeof fixConflicts>[1] | null = null;
    if (hadExistingSetup) {
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

    // ─── Step 8: Oh-My-OpenCode compatibility ───────────
    // Intentional: this branch handles the FIRST-TIME-INSTALL case only.
    // Existing users hit the same OMO conflict-fix logic via the
    // `if (hadExistingSetup) detectConflicts/fixConflicts` block above,
    // which already covers omoPreemptiveCompaction,
    // omoContextWindowMonitor, and omoAnthropicRecovery. Audit tools
    // sometimes flag this `!hadExistingSetup` gate as "OMO check skipped
    // for existing users" — that's a false positive.
    let disableOmoHooks = false;
    if (paths.omoConfig && !hadExistingSetup) {
        log.warn(`Found oh-my-opencode config: ${paths.omoConfig}`);
        log.message(
            "These hooks may conflict:\n" +
                "  • context-window-monitor\n" +
                "  • preemptive-compaction\n" +
                "  • anthropic-context-window-limit-recovery",
        );

        const shouldDisable = dryRun
            ? false
            : await confirm("Disable these hooks in oh-my-opencode?", true);
        if (dryRun) {
            log.message("[dry-run] would offer to disable conflicting oh-my-opencode hooks");
        }
        if (shouldDisable) {
            disableOmoHooks = true;
        } else {
            log.warn("Skipped — you may experience context management conflicts");
        }
    }

    if (!dryRun) {
        addPluginToOpenCodeConfig(
            paths.opencodeConfig,
            paths.opencodeConfigFormat,
            removeDcp,
            compactionEnabled,
        );
        log.success(`Plugin added to ${paths.opencodeConfig}`);
        if (removeDcp) log.success("Removed opencode-dcp from plugin list");
        if (compactionEnabled) {
            log.info("Disabled built-in compaction (auto=false, prune=false)");
            log.message("Eidnara handles context management — built-in compaction would interfere");
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
    }

    const summary = [
        `Plugin: ${PLUGIN_NAME}`,
        compactionEnabled
            ? "Compaction: disabled (Eidnara manages the window)"
            : "Compaction: off (native compaction owns the window)",
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

    outro("Run 'opencode' to start!");

    return 0;
}
