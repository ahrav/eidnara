import { existsSync, mkdirSync } from "node:fs";
import { basename, dirname } from "node:path";
import {
    loadProjectTierAdmission,
    loadUserTierConfigDetailed,
    loadUserTierConfigText,
} from "@eidnara/opencode/config";
import { resolveEidnaraProjectConfigPath } from "@eidnara/opencode/config/config-paths";
import { normalizeSummarizerChain } from "@eidnara/opencode/config/fold-authority";
import {
    type CompactionPatch,
    type ConflictResult,
    conflictDisposition,
    DCP_CONFLICT_REASON,
    detectConflicts,
    hasOmoPlugin,
    openCodeConfigLayerPaths,
    pluginEntriesOutside,
} from "@eidnara/opencode/shared/conflict-detector";
import { collectOmoConfigPaths, fixConflicts } from "@eidnara/opencode/shared/conflict-fixer";
import {
    appendJsoncArrayValues,
    removeJsoncArrayEntries,
    setJsoncValue,
} from "@eidnara/opencode/shared/jsonc-edit";
import { isRecord } from "@eidnara/opencode/shared/record-type-guard";
import { readRegularFileSync } from "@eidnara/opencode/shared/regular-file";
import { stringify as stringifyJsonc } from "comment-json";
import {
    isDevPathPluginEntry,
    isLocalPathPluginEntry,
    matchesPluginEntry,
} from "../adapters/opencode";
import { type AgentBlockKind, pruneInvalidAgentFields } from "../lib/agent-config";
import { writeFileAtomic } from "../lib/atomic-write";
import {
    describeFoldAuthority,
    type EidnaraModes,
    foldAuthorityOf,
    projectModeOverrides,
    readEidnaraModes,
} from "../lib/eidnara-modes";
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
import { log, type PromptIO, type PromptLog, promptIO } from "../lib/prompts";
import { compareVersionStrings } from "../lib/version";

const PLUGIN_NAME = "@eidnara/opencode";
const DCP_PLUGIN_NAME = "@tarquinen/opencode-dcp";
/** Mirrors the `@opencode-ai/plugin` peer dependency in `packages/opencode-plugin/package.json`. */
export const OPENCODE_MINIMUM_VERSION = "1.15.0";

/** With `enabled: false` the plugin skips every hook at startup, so native compaction must stay on. */
function resolveWriterModes(
    sharedConfigPath: string,
    directory: string,
    log: PromptLog,
): EidnaraModes {
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
    /** The `compaction` keys to write and their target values; keys it omits are left as found. */
    compaction: CompactionPatch = EIDNARA_FOLDS_COMPACTION,
    /** Plugin entries already effective from other config layers, such as a project config's dev path; an Eidnara entry among them suppresses the global one so OpenCode does not load the plugin twice. */
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
        if (Object.keys(compaction).length > 0) {
            created.compaction = { ...compaction };
        }
        writeFileAtomic(configPath, `${stringifyJsonc(created, null, 2)}\n`);
        return;
    }

    let text = readRegularFileSync(configPath);
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

    const targets = Object.entries(compaction);
    if (targets.length > 0) {
        if (!isRecord(existing.compaction)) {
            text = setJsoncValue(text, ["compaction"], { ...compaction });
            changed = true;
        } else {
            for (const [key, value] of targets) {
                if (existing.compaction[key] === value) continue;
                text = setJsoncValue(text, ["compaction", key], value);
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
        ? appendJsoncArrayValues(readRegularFileSync(configPath), ["plugin"], [PLUGIN_NAME])
        : setJsoncValue(readRegularFileSync(configPath), ["plugin"], [PLUGIN_NAME]);
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
    { log, confirm }: Pick<PromptIO, "log" | "confirm">,
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
    const conflicts = { ...result.conflicts, dcpPlugin: false };
    return {
        ...result,
        disposition: conflictDisposition(conflicts),
        reasons: result.reasons.filter((reason) => reason !== DCP_CONFLICT_REASON),
        conflicts,
    };
}

export const EIDNARA_FOLDS_COMPACTION: CompactionPatch = { auto: false, prune: false };
export const NATIVE_FOLDS_COMPACTION: CompactionPatch = { auto: true };

/** `keep` leaves every summarizer chain key as found; `remove` deletes all four. */
export type SummarizerChoice =
    | { kind: "model"; model: string }
    | { kind: "keep" }
    | { kind: "remove" };

const SUMMARIZER_CHAIN_KEYS = [
    "model",
    "fallback_models",
    "module_model",
    "module_fallback_models",
] as const;

export interface EidnaraConfigOptions {
    summarizer: SummarizerChoice;
    context_researcherEnabled: boolean;
    context_researcherModel: string | null;
    claudeMax: boolean;
}

/** Returns the exact text setup would write to `configPath`. */
export function proposeEidnaraConfig(configPath: string, options: EidnaraConfigOptions): string {
    const config = readJsoncConfigForUpdate(configPath);
    const summarizerModel = options.summarizer.kind === "model" ? options.summarizer.model : null;

    if (!config.$schema) {
        config.$schema =
            "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json";
    }

    if (options.summarizer.kind === "remove" && isRecord(config.history_summarizer)) {
        for (const key of SUMMARIZER_CHAIN_KEYS) delete config.history_summarizer[key];
    }
    if (summarizerModel) {
        const history_summarizer = asPlainRecord(config.history_summarizer);
        history_summarizer.model = summarizerModel;
        delete history_summarizer.disable;
        delete history_summarizer.enabled;
        warnPrunedAgentFields(
            configPath,
            "history_summarizer",
            pruneInvalidAgentFields("history_summarizer", history_summarizer),
        );
        config.history_summarizer = history_summarizer;
    }

    const context_researcher = asPlainRecord(config.context_researcher);
    delete context_researcher.enabled;
    if (options.context_researcherEnabled) {
        delete context_researcher.disable;
        if (options.context_researcherModel) {
            context_researcher.model = options.context_researcherModel;
        }
    } else {
        context_researcher.disable = true;
    }
    warnPrunedAgentFields(
        configPath,
        "context-researcher",
        pruneInvalidAgentFields("context-researcher", context_researcher),
    );
    config.context_researcher = context_researcher;

    if (options.claudeMax) {
        config.cache_ttl = withClaudeMaxCacheTtl(config.cache_ttl, [
            summarizerModel,
            options.context_researcherModel,
        ]);
    }

    return `${stringifyJsonc(config, null, 2)}\n`;
}

/**
 * A parseable config can still hold a schema-invalid block such as `"history_summarizer": "old-model"`; config loading logs "invalid agent configuration, ignoring" for it, and the writer starts fresh the same way.
 * A plain object is returned as is because comment-json keeps a block's comments as symbol-keyed metadata that a spread copy loses.
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
 * `detectConflicts` and `fixConflicts` skip unparseable files, so these repair targets are checked before any write.
 * Only the effective member of each project `.jsonc`/`.json` pair is listed,
 * matching the file OpenCode loads, so a stale shadowed sibling cannot block setup.
 * OMO files count only when a conflict repair can reach them; the caller
 * decides that from the enabled mode, the OMO plugin entry, and the
 * first-time branch.
 */
/**
 * Shared Eidnara config can come from Pi or OMP; only OpenCode config establishes an OpenCode setup.
 * Every layer the host loads counts (user, `OPENCODE_CONFIG`, project files, inline
 * `OPENCODE_CONFIG_CONTENT`), because `detectConflicts` reads DCP and OMO entries from all of them.
 */
export function hasExistingOpenCodeSetup(
    paths: Pick<ConfigPaths, "opencodeConfigFormat" | "tuiConfigFormat">,
    directory: string,
): boolean {
    return (
        paths.opencodeConfigFormat !== "none" ||
        paths.tuiConfigFormat !== "none" ||
        openCodeConfigLayerPaths(directory).some((path) => existsSync(path)) ||
        Boolean(process.env.OPENCODE_CONFIG_CONTENT?.trim())
    );
}

/**
 * Reports the conflicts a re-detection still finds after a repair. The fixer edits only files
 * that exist and that its editor accepts, so an accepted repair can leave a conflict in place (an
 * OMO plugin entry with no OMO config file, or a config the editor refused). Returns whether any
 * conflict remains.
 */
export function reportRemainingConflicts(
    remaining: ConflictResult,
    output: Pick<typeof log, "warn" | "message"> = log,
): boolean {
    if (remaining.disposition === "none") return false;
    output.warn(
        remaining.disposition === "disable"
            ? "Conflicts remain after the automatic fixes; Eidnara stays disabled until they are resolved:"
            : "Conflicts remain after the automatic fixes; Eidnara runs with a warning until they are resolved:",
    );
    for (const reason of [...remaining.reasons, ...remaining.unresolved]) {
        output.message(`  • ${reason}`);
    }
    output.message(
        "For oh-my-opencode without a config file, add `disabled_hooks` (context-window-monitor, preemptive-compaction, anthropic-context-window-limit-recovery) to its config, then rerun setup.",
    );
    return true;
}

export function preflightConfigPaths(
    paths: ConfigPaths & { eidnaraConfig: string },
    directory: string,
    options: { omoRepairReachable: boolean },
): string[] {
    // Every OpenCode layer the host loads and that exists (the other user sibling, `OPENCODE_CONFIG`,
    // project files) is checked, since a malformed one breaks the host whether or not setup writes it.
    const loadedLayers = openCodeConfigLayerPaths(directory).filter(
        (path) => path !== paths.opencodeConfig && existsSync(path),
    );
    return [
        paths.opencodeConfig,
        paths.eidnaraConfig,
        paths.tuiConfig,
        ...loadedLayers,
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

export interface SetupDependencies {
    io: PromptIO;
    /** `betweenWrites` runs between the OpenCode and Eidnara config writes, regardless of write order. */
    betweenWrites?: () => void;
}

export async function runSetup(
    dryRun = false,
    { io, betweenWrites }: SetupDependencies = { io: promptIO },
): Promise<number> {
    const { log } = io;
    const confirm = (message: string, defaultYes?: boolean) => io.confirm(message, defaultYes);
    io.intro("Eidnara — Setup");
    if (dryRun) {
        log.warn("Dry run — no files will be written and no config will be changed.");
    }

    const s = io.spinner();
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
            io.outro("Setup cancelled");
            return 1;
        }
    } else if (detection.kind === "desktop") {
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
                io.outro("Setup cancelled — upgrade OpenCode and try again.");
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
        io.outro("Setup stopped.");
        return 1;
    }
    // The guard above narrows the user config path for every write and preflight that follows.
    const paths = { ...detected, eidnaraConfig: detected.eidnaraConfig };
    const hadExistingSetup = hasExistingOpenCodeSetup(paths, process.cwd());
    // With Eidnara disabled nothing conflicts, so no conflict repair is offered in that mode.
    const modes = resolveWriterModes(paths.eidnaraConfig, process.cwd(), log);
    const omoConfigs = collectOmoConfigPaths(process.cwd());
    const omoReachableWhenEnabled =
        hasOmoPlugin(process.cwd()) || (omoConfigs.length > 0 && !hadExistingSetup);

    // The preflight is read-only, so a dry run performs it too and predicts the refusal a real run would make.
    try {
        assertJsoncConfigsParseable(
            preflightConfigPaths(paths, process.cwd(), {
                omoRepairReachable: modes.enabled && omoReachableWhenEnabled,
            }),
        );
        assertPluginListShape([paths.opencodeConfig, paths.tuiConfig]);
    } catch (error) {
        log.error(error instanceof Error ? error.message : String(error));
        io.outro("Setup stopped — fix the malformed config and rerun setup.");
        return 1;
    }

    const existingChain = normalizeSummarizerChain(
        loadUserTierConfigDetailed(paths.eidnaraConfig).config.history_summarizer,
    );
    const summarizer = await pickSummarizer(io, allModels, existingChain);
    const summarizerModel = summarizer.kind === "model" ? summarizer.model : null;

    const context_researcherEnabled = await confirm("Enable context_researcher?", false);
    let context_researcherModel: string | null = null;
    if (context_researcherEnabled) {
        context_researcherModel = await pickModel(io, allModels, "context-researcher");
        log.success(`ContextResearcher: ${context_researcherModel}`);
    }

    const hasAnthropic = hasAnthropicModel([
        ...allModels,
        summarizerModel,
        context_researcherModel,
    ]);
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

    // The fold authority comes from the exact document setup writes, validated the way the plugin
    // loads it, before any conflict question or host edit.
    const source = readSourceText(paths.eidnaraConfig);
    const proposal = proposeEidnaraConfig(paths.eidnaraConfig, {
        summarizer,
        context_researcherEnabled,
        context_researcherModel,
        claudeMax,
    });
    const proposed = loadUserTierConfigText(paths.eidnaraConfig, proposal);
    const authority = foldAuthorityOf(proposed);
    const enabled = proposed.config.enabled !== false;
    const firstTimeOmoRepair = enabled && omoConfigs.length > 0 && !hadExistingSetup;
    const omoRepairReachable = enabled && omoReachableWhenEnabled;
    log.info(`Fold authority: ${describeFoldAuthority(authority)}`);
    const projectAdmission = loadProjectTierAdmission(process.cwd());
    const rejection =
        authority.kind === "unresolved"
            ? `the proposed ${paths.eidnaraConfig} does not load: ${authority.reason}`
            : projectAdmission.status === "unresolved"
              ? `the project Eidnara config does not load: ${projectAdmission.reason}`
              : null;
    if (rejection !== null || authority.kind === "unresolved") {
        log.error(`Setup edits no host setting because ${rejection}`);
        io.outro("Setup stopped — fix the Eidnara config and rerun setup.");
        return 1;
    }
    if (omoRepairReachable && !modes.enabled) {
        try {
            assertJsoncConfigsParseable(omoConfigs);
        } catch (error) {
            log.error(error instanceof Error ? error.message : String(error));
            io.outro("Setup stopped — fix the malformed config and rerun setup.");
            return 1;
        }
    }
    if (dryRun) {
        log.message(`[dry-run] proposed ${paths.eidnaraConfig}:\n${proposal}`);
    }

    const dcpDecision: DcpDecision =
        dryRun || !enabled
            ? "absent"
            : await resolveDcpConflictBeforeSetup(
                  paths.opencodeConfig,
                  paths.opencodeConfigFormat,
                  io,
              );
    const removeDcp = dcpDecision === "remove";
    const eidnaraFolds = authority.kind === "eidnara";
    const compactionTarget: CompactionPatch = !enabled
        ? {}
        : eidnaraFolds
          ? EIDNARA_FOLDS_COMPACTION
          : NATIVE_FOLDS_COMPACTION;

    let conflictFix: ConflictResult | null = null;
    // A declined fix covers the native compaction flags too; the writer must not apply them anyway.
    let keepNativeCompaction = false;
    if (hadExistingSetup && enabled) {
        const detected = detectConflicts(process.cwd(), { compactionEnabled: eidnaraFolds });
        const conflicts = dcpDecision === "keep" ? withoutDcpConflict(detected) : detected;
        if (conflicts.disposition !== "none") {
            log.warn(
                conflicts.disposition === "disable"
                    ? "Found conflicting configuration that can disable Eidnara:"
                    : "Found configuration that leaves the session without a fold authority:",
            );
            for (const reason of [...conflicts.reasons, ...conflicts.unresolved]) {
                log.message(`  • ${reason}`);
            }

            if (dryRun) {
                log.message("[dry-run] would offer to apply automatic conflict fixes");
            } else if (
                await confirm(
                    "Apply automatic conflict fixes to your OpenCode and OMO config files?",
                    true,
                )
            ) {
                conflictFix = conflicts;
            } else {
                const { compactionAuto, compactionPrune, noFoldAuthority } = conflicts.conflicts;
                keepNativeCompaction = compactionAuto || compactionPrune || noFoldAuthority;
                log.warn(
                    "Skipped automatic conflict fixes — the fold authority stays as configured and OpenCode's settings stay as they are",
                );
            }
        }
    }
    const hostCompaction = keepNativeCompaction ? {} : compactionTarget;

    if (dryRun) {
        log.message(
            `[dry-run] would add the plugin to ${paths.opencodeConfig}; ${describeCompactionWrite(hostCompaction)}`,
        );
        log.message(
            `[dry-run] would write Eidnara config to ${paths.eidnaraConfig} (${describeFoldAuthority(authority)})`,
        );
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

    let repairIncomplete = false;
    if (!dryRun && readSourceText(paths.eidnaraConfig) !== source) {
        log.error(
            `${paths.eidnaraConfig} changed while setup was running; setup wrote nothing so that edit is kept.`,
        );
        io.outro("Setup stopped — rerun setup to build a proposal from the current file.");
        return 1;
    }
    const finalProjectAdmission = dryRun
        ? projectAdmission
        : loadProjectTierAdmission(process.cwd());
    if (finalProjectAdmission.status === "unresolved") {
        log.error(
            `Setup edits no host setting because the project Eidnara config does not load: ${finalProjectAdmission.reason}`,
        );
        io.outro("Setup stopped — fix the Eidnara config and rerun setup.");
        return 1;
    }
    if (!dryRun) {
        // Every file a later step may write is captured first, so a failure part-way (a read-only
        // directory, for example) restores the OpenCode registration and compaction flags instead
        // of leaving the plugin active without its config.
        let snapshot: ReturnType<typeof snapshotFiles>;
        try {
            snapshot = snapshotFiles([
                ...preflightConfigPaths(paths, process.cwd(), { omoRepairReachable }),
                ...openCodeConfigLayerPaths(process.cwd()),
            ]);
        } catch (error) {
            log.error(error instanceof Error ? error.message : String(error));
            io.outro("Setup stopped before writing — make the file readable, then rerun setup.");
            return 1;
        }
        try {
            const writeHost = () => {
                addPluginToOpenCodeConfig(
                    paths.opencodeConfig,
                    paths.opencodeConfigFormat,
                    removeDcp,
                    hostCompaction,
                    pluginEntriesOutside(process.cwd(), paths.opencodeConfig),
                );
                log.success(`Plugin added to ${paths.opencodeConfig}`);
                if (removeDcp) log.success("Removed opencode-dcp from plugin list");
                log.info(describeCompactionWrite(hostCompaction));

                if (conflictFix) {
                    const actions = fixConflicts(process.cwd(), conflictFix);
                    if (actions.length > 0) {
                        for (const action of actions) log.success(action);
                    } else {
                        log.info("No additional conflict changes were needed");
                    }
                }
            };
            const writeEidnara = () => {
                writeFileAtomic(paths.eidnaraConfig, proposal);
                log.success(`Config written to ${paths.eidnaraConfig}`);
            };
            // A process killed between config writes leaves the first write persisted. Under Eidnara
            // folds the summarizer chain is written before `compaction.auto=false`, so that state
            // leaves OpenCode's previous compaction setting in effect alongside the new chain.
            if (eidnaraFolds) {
                writeEidnara();
                betweenWrites?.();
                writeHost();
            } else {
                writeHost();
                betweenWrites?.();
                writeEidnara();
            }
            addPluginToTuiConfig(paths.tuiConfig, paths.tuiConfigFormat);
            log.success(`TUI sidebar plugin added to ${basename(paths.tuiConfig)}`);

            if (disableOmoHooks) {
                const actions = fixConflicts(process.cwd(), {
                    conflicts: {
                        compactionAuto: false,
                        compactionPrune: false,
                        noFoldAuthority: false,
                        dcpPlugin: false,
                        omoPreemptiveCompaction: true,
                        omoContextWindowMonitor: true,
                        omoAnthropicRecovery: true,
                    },
                    compactionPatch: {},
                });
                if (actions.includes("Disabled conflicting oh-my-opencode hooks")) {
                    log.success("Hooks disabled in oh-my-opencode config");
                }
            }
        } catch (error) {
            log.error(error instanceof Error ? error.message : String(error));
            let rolledBack = true;
            try {
                restoreFiles(snapshot);
            } catch (rollbackError) {
                rolledBack = false;
                log.error(
                    rollbackError instanceof Error ? rollbackError.message : String(rollbackError),
                );
            }
            reportReadBack(paths, proposal, hostCompaction, log);
            io.outro(
                rolledBack
                    ? "Setup stopped — rolled back OpenCode changes."
                    : "Setup stopped — changes were only partly rolled back; restore the files above by hand.",
            );
            return 1;
        }
        // The editor refuses some parseable files (duplicate keys, for one), so an accepted repair
        // can write nothing; re-detection reports what the files still hold.
        if (
            enabled &&
            reportRemainingConflicts(
                detectConflicts(process.cwd(), { compactionEnabled: eidnaraFolds }),
                { warn: log.warn, message: log.message },
            )
        ) {
            repairIncomplete = true;
        }
        if (reportReadBack(paths, proposal, hostCompaction, log)) {
            log.info("Written, restart required: OpenCode reads these files at startup.");
        } else {
            repairIncomplete = true;
        }
    }

    const summary = [
        `Plugin: ${PLUGIN_NAME}`,
        `Fold authority: ${describeFoldAuthority(authority)}`,
        `Compaction: ${describeCompactionWrite(hostCompaction)}`,
        summarizer.kind === "model"
            ? `HistorySummarizer: ${summarizer.model}`
            : summarizer.kind === "keep"
              ? "HistorySummarizer: existing chain kept"
              : "HistorySummarizer: none",
        context_researcherEnabled
            ? `ContextResearcher: enabled${context_researcherModel ? ` (${context_researcherModel})` : ""}`
            : "ContextResearcher: disabled",
    ].join("\n");

    io.note(summary, dryRun ? "Configuration (dry run — not written)" : "Configuration");

    if (dryRun) {
        io.outro("Dry run complete — nothing was written.");
        return 0;
    }

    if (repairIncomplete) {
        io.outro(
            "Setup finished with warnings — resolve the remaining conflicts, then restart OpenCode.",
        );
        return 1;
    }
    io.outro("Written; restart OpenCode to apply the changes.");

    return 0;
}

function readSourceText(path: string): string | null {
    return existsSync(path) ? readRegularFileSync(path) : null;
}

function describeCompactionWrite(compaction: CompactionPatch): string {
    if (compaction.auto === false) {
        return "OpenCode compaction.auto=false and compaction.prune=false (Eidnara folds; OpenCode's compaction would interfere)";
    }
    if (compaction.auto === true) {
        return "OpenCode compaction.auto=true, compaction.prune left as found (OpenCode's native compaction folds; prune is OpenCode's own tool-output policy)";
    }
    return "OpenCode compaction settings left unchanged";
}

function reportReadBack(
    paths: Pick<ConfigPaths, "opencodeConfig"> & { eidnaraConfig: string },
    proposal: string,
    compaction: CompactionPatch,
    log: PromptLog,
): boolean {
    const written = foldAuthorityOf(loadUserTierConfigDetailed(paths.eidnaraConfig));
    const host = readJsoncLenient(paths.opencodeConfig).value.compaction;
    const hostFields = isRecord(host) ? host : {};
    const hostMatches = Object.entries(compaction).every(
        ([key, value]) => hostFields[key] === value,
    );
    const eidnaraMatches = readSourceText(paths.eidnaraConfig) === proposal;
    log.message(
        `Read back: ${paths.eidnaraConfig} → ${describeFoldAuthority(written)}; ${paths.opencodeConfig} → compaction.auto=${String(hostFields.auto ?? "unset")}, compaction.prune=${String(hostFields.prune ?? "unset")}`,
    );
    if (eidnaraMatches && hostMatches) return true;
    log.warn(
        "The files do not both hold the proposed settings; the next OpenCode start runs with the read-back values above. Rerun setup, or run `eidnara doctor`.",
    );
    return false;
}

async function pickSummarizer(
    io: PromptIO,
    allModels: string[],
    existingChain: readonly string[],
): Promise<SummarizerChoice> {
    const choice = await io.selectOne("History summarizer", [
        { label: "Choose a summarizer model (Eidnara folds)", value: "model", recommended: true },
        {
            label:
                existingChain.length > 0
                    ? `Keep the existing summarizer chain (${existingChain.join(", ")})`
                    : "Keep the existing summarizer settings (no model is configured)",
            value: "keep",
        },
        {
            label: "No summarizer: remove every summarizer model field (OpenCode's native compaction folds)",
            value: "remove",
        },
    ]);
    if (choice === "keep") return { kind: "keep" };
    if (choice === "remove") return { kind: "remove" };
    const model = await pickModel(io, allModels, "history_summarizer");
    io.log.success(`HistorySummarizer: ${model}`);
    return { kind: "model", model };
}
