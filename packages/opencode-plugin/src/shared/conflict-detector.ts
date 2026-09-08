import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { detectConfigFile, parseConfigJsonc, readJsoncFile } from "./jsonc-parser";
import { log } from "./logger";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";
import { isRecord } from "./record-type-guard";

/** `readJsoncFile` does not validate parsed JSON, so consumers check runtime types before iterating. */
interface OpenCodeConfig {
    compaction?: unknown;
    plugin?: unknown;
}

export interface ConflictResult {
    /* */
    hasConflict: boolean;
    /** Each `reasons` entry describes a conflict in human-readable text. */
    reasons: string[];
    /* */
    conflicts: {
        compactionAuto: boolean;
        compactionPrune: boolean;
        dcpPlugin: boolean;
        omoPreemptiveCompaction: boolean;
        omoContextWindowMonitor: boolean;
        omoAnthropicRecovery: boolean;
    };
    /**
     * `nativeCompaction` records the resolved native compaction state observed during detection.
     * `auto` and `prune` reflect the detector's resolved OpenCode compaction state.
     * `auto` and `prune` are populated when Eidnara compaction is off.
     * `auto` and `prune` are not conflicts when Eidnara compaction is off.
     */
    nativeCompaction: {
        auto: boolean;
        prune: boolean;
    };
}

/**
 * The host reports resolved native compaction state through `ctx.client.config.get()`.
 * `ctx.client.config.get()` returns the object that `opencode debug config` prints.
 * `auto` defaults to `true`; `prune` defaults to `false`.
 * `compaction` defaults to `{ auto: true, prune: false }`.
 */
export interface ResolvedCompaction {
    auto: boolean;
    prune: boolean;
}

/**
 *
 * `compactionEnabled` is the boot-resolved Eidnara compaction mode
 * Call sites with a Eidnara config handle must pass `compactionEnabled`.
 * Plugin boot, setup, doctor, and conflict-fixer must not re-derive the mode.
 * A call site without a Eidnara config handle must omit `compactionEnabled`.
 *
 * `resolvedCompaction` is the host's RESOLVED native compaction state
 * `resolveCompactionForBoot` fetches `resolvedCompaction`.
 *
 * When `compactionEnabled` is `false`, native `compaction.auto` and `compaction.prune` are not conflicts.
 * `compaction.auto=true` and `compaction.prune=true` do not disable Eidnara.
 * DCP and the three OMO conflict classes remain conflicts in both modes.
 * modes.
 */
export interface DetectConflictsOptions {
    compactionEnabled?: boolean;
    resolvedCompaction?: ResolvedCompaction;
}

/** The `reasons` entry `detectConflicts` emits for `conflicts.dcpPlugin`. */
export const DCP_CONFLICT_REASON =
    "opencode-dcp plugin is installed — it conflicts with Eidnara's context management";

/**
 *
 *
 */
export function detectConflicts(
    directory: string,
    options?: DetectConflictsOptions,
): ConflictResult {
    const compactionEnabled = options?.compactionEnabled ?? true;
    const conflicts: ConflictResult["conflicts"] = {
        compactionAuto: false,
        compactionPrune: false,
        dcpPlugin: false,
        omoPreemptiveCompaction: false,
        omoContextWindowMonitor: false,
        omoAnthropicRecovery: false,
    };
    const reasons: string[] = [];

    const compactionResult = options?.resolvedCompaction ?? checkCompaction(directory);
    if (compactionEnabled && compactionResult.auto) {
        conflicts.compactionAuto = true;
        reasons.push(
            options?.resolvedCompaction
                ? "OpenCode auto-compaction is enabled (compaction.auto=true) (resolved config)"
                : "OpenCode auto-compaction is enabled (compaction.auto=true)",
        );
    }
    if (compactionEnabled && compactionResult.prune) {
        conflicts.compactionPrune = true;
        reasons.push(
            options?.resolvedCompaction
                ? "OpenCode prune is enabled (compaction.prune=true) (resolved config)"
                : "OpenCode prune is enabled (compaction.prune=true)",
        );
    }

    const dcpFound = checkDcpPlugin(directory);
    if (dcpFound) {
        conflicts.dcpPlugin = true;
        reasons.push(DCP_CONFLICT_REASON);
    }

    const omoResult = checkOmoHooks(directory);
    if (omoResult.preemptiveCompaction) {
        conflicts.omoPreemptiveCompaction = true;
        reasons.push(
            "oh-my-opencode preemptive-compaction hook is active — it triggers compaction that conflicts with historian",
        );
    }
    if (omoResult.contextWindowMonitor) {
        conflicts.omoContextWindowMonitor = true;
        reasons.push(
            "oh-my-opencode context-window-monitor hook is active — it injects usage warnings that overlap with Eidnara nudges",
        );
    }
    if (omoResult.anthropicRecovery) {
        conflicts.omoAnthropicRecovery = true;
        reasons.push(
            "oh-my-opencode anthropic-context-window-limit-recovery hook is active — it triggers emergency compaction that bypasses historian",
        );
    }

    return {
        hasConflict: reasons.length > 0,
        reasons,
        conflicts,
        nativeCompaction: { auto: compactionResult.auto, prune: compactionResult.prune },
    };
}

/**
 * The SDK-generated `Config` type omits `compaction`; read it from the runtime response.
 */
export interface OpencodeConfigClientLike {
    config: {
        get: () => Promise<{ data?: unknown }>;
    };
}

/**
 */
interface ResolvedCompactionBlock {
    compaction?: {
        auto?: boolean;
        prune?: boolean;
    };
}

/**
 * `client.config.get()` provides the host's resolved compaction state.
 * The conflict decision uses the host's resolved config because file-based detection cannot observe every config layer.
 * File-based detection would wrongly flag `auto=false` set in an unread config layer.
 * us.
 *
 * A response without `compaction` is inconclusive.
 * Treating an absent block as `auto=true` can disable plugin compaction.
 * Only explicit host booleans resolve compaction state; otherwise return `null` and use file-based detection.
 *
 * The function returns `null` when the fetch fails, times out after `timeoutMs`, or has no explicit compaction block.
 */
export async function resolveCompactionForBoot(
    client: OpencodeConfigClientLike,
    timeoutMs = 2_000,
): Promise<ResolvedCompaction | null> {
    // A pending timer keeps the event loop alive, so a fetch that wins the race must still
    // clear it or a short-lived caller waits out `timeoutMs` before exiting.
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
        const result = await Promise.race([
            client.config.get(),
            new Promise<never>((_, reject) => {
                timer = setTimeout(() => reject(new Error("config.get() timed out")), timeoutMs);
            }),
        ]);
        // The SDK's generated `Config` type omits `compaction`, so the function reads `compaction` from the runtime response.
        const compaction = (result?.data as ResolvedCompactionBlock | undefined)?.compaction;
        // The function returns `null` unless `compaction.auto` and `compaction.prune` are booleans.
        if (typeof compaction?.auto !== "boolean" || typeof compaction?.prune !== "boolean") {
            log(
                `[eidnara] conflict-detector: resolved config carried no explicit compaction block (${JSON.stringify(compaction) ?? "absent"}); falling back to file-based detection`,
            );
            return null;
        }
        return { auto: compaction.auto, prune: compaction.prune };
    } catch {
        return null;
    } finally {
        if (timer !== undefined) clearTimeout(timer);
    }
}

/**
 * Mirrors the host's flag rule: only `"true"` or `"1"`, case-insensitively, enables a flag.
 */
function hostFlagEnabled(
    name:
        | "OPENCODE_DISABLE_AUTOCOMPACT"
        | "OPENCODE_DISABLE_PRUNE"
        | "OPENCODE_DISABLE_PROJECT_CONFIG",
): boolean {
    const value = process.env[name]?.toLowerCase();
    return value === "true" || value === "1";
}

/**
 * OpenCode config files in host merge order, lowest precedence first: user-level `opencode.json`, user-level `opencode.jsonc`, the `OPENCODE_CONFIG` file, project root, then `.opencode/`. commentlint: allow(JUDGE)
 * Later entries override earlier ones key by key. The list contains candidate paths; callers decide whether a missing file matters. commentlint: allow(JUDGE)
 * `OPENCODE_DISABLE_PROJECT_CONFIG` omits the project-root and `.opencode/` layers. commentlint: allow(JUDGE)
 */
export function openCodeConfigLayerPaths(directory: string): string[] {
    const user = getOpenCodeConfigPaths({ binary: "opencode" });
    const layers = [user.configJson, user.configJsonc];
    const customConfig = process.env.OPENCODE_CONFIG;
    if (customConfig) layers.push(customConfig);
    if (!hostFlagEnabled("OPENCODE_DISABLE_PROJECT_CONFIG")) {
        layers.push(
            join(directory, "opencode.json"),
            join(directory, "opencode.jsonc"),
            join(directory, ".opencode", "opencode.json"),
            join(directory, ".opencode", "opencode.jsonc"),
        );
    }
    return layers;
}

/**
 * `.opencode/` paths precede project-root paths; `.jsonc` precedes `.json` in each directory.
 * All four paths are returned regardless of file existence; callers probe for existence.
 */
export function projectOpenCodeConfigPaths(
    directory: string,
): readonly [string, string, string, string] {
    return [
        join(directory, ".opencode", "opencode.jsonc"),
        join(directory, ".opencode", "opencode.json"),
        join(directory, "opencode.jsonc"),
        join(directory, "opencode.json"),
    ];
}

/**
 * The host merges the inline `OPENCODE_CONFIG_CONTENT` JSON after every file layer, so it
 * is the highest-precedence entry here. Missing, unparseable, and non-object layers are skipped.
 */
function readOpenCodeConfigLayers(directory: string): OpenCodeConfig[] {
    const layers: OpenCodeConfig[] = [];
    for (const configPath of openCodeConfigLayerPaths(directory)) {
        const config = readJsoncFile<unknown>(configPath);
        if (isRecord(config)) layers.push(config);
    }
    const inline = process.env.OPENCODE_CONFIG_CONTENT;
    if (inline) {
        try {
            const config = parseConfigJsonc<unknown>(inline);
            if (isRecord(config)) layers.push(config);
        } catch {
            /* The host rejects the same malformed content, so it contributes nothing. */
        }
    }
    return layers;
}

/**
 * Deep-merges `compaction` across every layer the host reads and applies the host
 * defaults (`auto: true`, `prune: false`) only to keys no layer set. Non-boolean values
 * are ignored rather than coerced. `OPENCODE_DISABLE_AUTOCOMPACT` and `OPENCODE_DISABLE_PRUNE`
 * are applied after the merge, where the host applies them, so each flag forces its own key
 * to `false` regardless of what any layer set.
 */
function checkCompaction(directory: string): { auto: boolean; prune: boolean } {
    let auto: boolean | undefined;
    let prune: boolean | undefined;

    for (const { compaction } of readOpenCodeConfigLayers(directory)) {
        if (!isRecord(compaction)) continue;
        if (typeof compaction.auto === "boolean") auto = compaction.auto;
        if (typeof compaction.prune === "boolean") prune = compaction.prune;
    }

    return {
        auto: hostFlagEnabled("OPENCODE_DISABLE_AUTOCOMPACT") ? false : (auto ?? true),
        prune: hostFlagEnabled("OPENCODE_DISABLE_PRUNE") ? false : (prune ?? false),
    };
}

/**
 * `DCP_PACKAGE_NAMES` lists canonical npm package names for the conflicting DCP plugin.
 *
 */
export const DCP_PACKAGE_NAMES = new Set(["@tarquinen/opencode-dcp"]);

function checkDcpPlugin(directory: string): boolean {
    const plugins = collectPluginEntries(directory);
    return plugins.some((p) => matchesPackageName(p, DCP_PACKAGE_NAMES));
}

/**
 *
 *   - "pkg-name"
 *   - "pkg-name@version"
 *   - "@scope/pkg-name"
 *   - "@scope/pkg-name@version"
 *
 */
export function matchesPackageName(entry: string, canonicalNames: Set<string>): boolean {
    // The canonical matcher skips URL and path entries because only npm-style entries have canonical package names.
    if (
        entry.startsWith("file:") ||
        entry.startsWith("http:") ||
        entry.startsWith("https:") ||
        entry.startsWith("/") ||
        entry.startsWith("./") ||
        entry.startsWith("../")
    ) {
        return false;
    }

    // The leading "@" belongs to scoped package names.
    const lastAt = entry.lastIndexOf("@");
    const nameOnly = lastAt > 0 ? entry.slice(0, lastAt) : entry;
    return canonicalNames.has(nameOnly);
}

/**
 * */
export function extractPluginName(entry: unknown): string | null {
    if (typeof entry === "string") return entry;
    if (Array.isArray(entry) && typeof entry[0] === "string") return entry[0];
    return null;
}

/** Keeps only string items so a malformed config value can be iterated without throwing. */
export function asStringArray(value: unknown): string[] {
    return Array.isArray(value)
        ? value.filter((item): item is string => typeof item === "string")
        : [];
}

/**
 * Raw `plugin` entries from the project-level OpenCode config files under `directory`, in
 * `projectOpenCodeConfigPaths` order. Entries keep their original shape so a caller can match
 * tuple options as well as names. `OPENCODE_DISABLE_PROJECT_CONFIG` yields an empty list because
 * the host loads none of these files then.
 */
/** Mirrors the host: `OPENCODE_DISABLE_PROJECT_CONFIG=true|1` removes every project config layer. */
export function projectConfigDisabled(): boolean {
    return hostFlagEnabled("OPENCODE_DISABLE_PROJECT_CONFIG");
}

export function projectPluginEntries(directory: string): unknown[] {
    if (projectConfigDisabled()) return [];
    const entries: unknown[] = [];
    for (const configPath of projectOpenCodeConfigPaths(directory)) {
        const config = readJsoncFile<unknown>(configPath);
        if (!isRecord(config) || !Array.isArray(config.plugin)) continue;
        entries.push(...config.plugin);
    }
    return entries;
}

/**
 * Raw `plugin` entries from every layer the host loads: the user config siblings,
 * `OPENCODE_CONFIG`, the project files, and inline `OPENCODE_CONFIG_CONTENT`. A writer passes the
 * file it is about to write as `excludePath` to see what is already registered elsewhere.
 */
export function pluginEntriesOutside(directory: string, excludePath?: string): unknown[] {
    const excluded = excludePath === undefined ? null : resolve(excludePath);
    const entries: unknown[] = [];
    for (const configPath of openCodeConfigLayerPaths(directory)) {
        if (excluded !== null && resolve(configPath) === excluded) continue;
        const config = readJsoncFile<unknown>(configPath);
        if (isRecord(config) && Array.isArray(config.plugin)) entries.push(...config.plugin);
    }
    const inline = process.env.OPENCODE_CONFIG_CONTENT;
    if (inline) {
        try {
            const config = parseConfigJsonc<unknown>(inline);
            if (isRecord(config) && Array.isArray(config.plugin)) entries.push(...config.plugin);
        } catch {
            /* The host rejects the same malformed content, so it contributes nothing. */
        }
    }
    return entries;
}

function collectPluginEntries(directory: string): string[] {
    const plugins: string[] = [];

    for (const { plugin: entries } of readOpenCodeConfigLayers(directory)) {
        if (!Array.isArray(entries)) continue;
        for (const entry of entries) {
            const name = extractPluginName(entry);
            if (name) plugins.push(name);
        }
    }

    return plugins;
}

/**
 *
 *
 */
const OMO_PACKAGE_NAMES = new Set(["oh-my-opencode", "oh-my-openagent"]);

/** Whether any OpenCode config layer the host loads lists an OMO plugin entry. */
export function hasOmoPlugin(directory: string): boolean {
    return collectPluginEntries(directory).some((p) => matchesPackageName(p, OMO_PACKAGE_NAMES));
}

/**
 * Hook names oh-my-opencode activates by default that overlap Eidnara's context
 * management, keyed by the `ConflictResult["conflicts"]` field they set.
 */
export const OMO_CONFLICTING_HOOKS = {
    omoContextWindowMonitor: "context-window-monitor",
    omoPreemptiveCompaction: "preemptive-compaction",
    omoAnthropicRecovery: "anthropic-context-window-limit-recovery",
} as const;

const OMO_LEGACY_CONFIG_BASENAMES = ["oh-my-openagent", "oh-my-opencode"] as const;

const OMO_UNIFIED_CONFIG_BASENAME = "omo";

export interface OmoConfigCandidate {
    path: string;
    /** Unified `omo.json[c]` files nest OpenCode settings under an `"[opencode]"` key. */
    unified: boolean;
}

/**
 * `basenames` is probed in order, and `detectConfigFile` prefers `.jsonc` over `.json`.
 * oh-my-opencode reads one file per location; a stale `.json` can mask a hook enabled by `.jsonc`. commentlint: allow(JUDGE)
 */
function activeOmoConfigFile(dir: string, basenames: readonly string[]): string | null {
    for (const basename of basenames) {
        const detected = detectConfigFile(join(dir, basename));
        if (detected.format !== "none") return detected.path;
    }
    return null;
}

/**
 * `homedir()` throws for a UID without a passwd entry when `HOME` is unset; that process has
 * no user-level `.omo` location, so the lookup reports none instead of failing.
 */
function userOmoDir(): string | null {
    if (process.env.HOME) return join(process.env.HOME, ".omo");
    try {
        return join(homedir(), ".omo");
    } catch {
        return null;
    }
}

/** Shared by the detector and the fixer so their read and write sets cannot drift. commentlint: allow(JUDGE) */
export function omoConfigCandidatePaths(directory: string): OmoConfigCandidate[] {
    const configDir = getOpenCodeConfigPaths({ binary: "opencode" }).configDir;
    const locations: Array<{ dir: string; basenames: readonly string[]; unified: boolean }> = [
        { dir: configDir, basenames: OMO_LEGACY_CONFIG_BASENAMES, unified: false },
        {
            dir: join(directory, ".opencode"),
            basenames: OMO_LEGACY_CONFIG_BASENAMES,
            unified: false,
        },
    ];
    const omoHomeDir = userOmoDir();
    if (omoHomeDir) {
        locations.push({
            dir: omoHomeDir,
            basenames: [OMO_UNIFIED_CONFIG_BASENAME],
            unified: true,
        });
    }
    locations.push({
        dir: join(directory, ".omo"),
        basenames: [OMO_UNIFIED_CONFIG_BASENAME],
        unified: true,
    });

    const candidates: OmoConfigCandidate[] = [];
    for (const { dir, basenames, unified } of locations) {
        const path = activeOmoConfigFile(dir, basenames);
        if (path) candidates.push({ path, unified });
    }
    return candidates;
}

function checkOmoHooks(directory: string): {
    preemptiveCompaction: boolean;
    contextWindowMonitor: boolean;
    anthropicRecovery: boolean;
} {
    const result = {
        preemptiveCompaction: false,
        contextWindowMonitor: false,
        anthropicRecovery: false,
    };

    if (!hasOmoPlugin(directory)) return result;

    const disabledHooks = readOmoDisabledHooks(directory);

    result.preemptiveCompaction = !disabledHooks.has(OMO_CONFLICTING_HOOKS.omoPreemptiveCompaction);
    result.contextWindowMonitor = !disabledHooks.has(OMO_CONFLICTING_HOOKS.omoContextWindowMonitor);
    result.anthropicRecovery = !disabledHooks.has(OMO_CONFLICTING_HOOKS.omoAnthropicRecovery);

    return result;
}

function readOmoDisabledHooks(directory: string): Set<string> {
    const disabled = new Set<string>();

    for (const candidate of omoConfigCandidatePaths(directory)) {
        const config = readJsoncFile<unknown>(candidate.path);
        if (!isRecord(config)) continue;
        const block = candidate.unified ? config["[opencode]"] : config;
        if (!isRecord(block)) continue;
        for (const hook of asStringArray(block.disabled_hooks)) {
            disabled.add(hook);
        }
    }

    return disabled;
}

/**
 */
export function formatConflictShort(result: ConflictResult): string {
    if (!result.hasConflict) return "";

    const lines = [
        "⚠️ Eidnara is disabled due to conflicting configuration:",
        "",
        ...result.reasons.map((r) => `• ${r}`),
        "",
        "Fix: run `eidnara doctor`",
    ];
    return lines.join("\n");
}
