import { homedir } from "node:os";
import { join } from "node:path";
import { readJsoncFile } from "./jsonc-parser";
import { log } from "./logger";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";

interface OpenCodeConfig {
    compaction?: {
        auto?: boolean;
        prune?: boolean;
    };
    // OpenCode allows plugins as plain strings or [name, options] tuples.
    plugin?: Array<string | [string, unknown]>;
}

interface OmoConfig {
    disabled_hooks?: string[];
}

/**
 *  Hook config lives inside the `[opencode]` harness block. */
interface OmoV2Config {
    "[opencode]"?: {
        disabled_hooks?: string[];
    };
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

    // Resolved config avoids treating an unseen `auto=false` layer as `auto=true`.
    let compactionResult = options?.resolvedCompaction ?? checkCompaction(directory);
    if (process.env.OPENCODE_DISABLE_AUTOCOMPACT) {
        compactionResult = { auto: false, prune: false };
    }
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
    try {
        const result = await Promise.race([
            client.config.get(),
            new Promise<never>((_, reject) =>
                setTimeout(() => reject(new Error("config.get() timed out")), timeoutMs),
            ),
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
    }
}

function checkCompaction(directory: string): { auto: boolean; prune: boolean } {
    if (process.env.OPENCODE_DISABLE_AUTOCOMPACT) {
        return { auto: false, prune: false };
    }

    // Project-level config takes precedence and is checked first.
    const projectResult = readProjectCompaction(directory);
    if (projectResult.resolved) return projectResult;

    const userResult = readUserCompaction();
    if (userResult.resolved) return userResult;

    return { auto: true, prune: false };
}

/** Project-level OpenCode config paths in precedence order, as `.jsonc`/`.json` pairs per directory. */
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

/** OpenCode and OMO load one config per directory, `.jsonc` first; a shadowed `.json` sibling is not consulted. */
function readEffectiveConfig<T = OpenCodeConfig>(jsoncPath: string, jsonPath: string): T | null {
    return readJsoncFile<T>(jsoncPath) ?? readJsoncFile<T>(jsonPath);
}

function readProjectCompaction(directory: string): {
    auto: boolean;
    prune: boolean;
    resolved: boolean;
} {
    const [dotOcJsonc, dotOcJson, rootJsonc, rootJson] = projectOpenCodeConfigPaths(directory);
    const dotOcConfig = readEffectiveConfig(dotOcJsonc, dotOcJson);

    if (dotOcConfig?.compaction) {
        const c = dotOcConfig.compaction;
        if (c.auto !== undefined || c.prune !== undefined) {
            return { auto: c.auto === true, prune: c.prune === true, resolved: true };
        }
    }

    const rootConfig = readEffectiveConfig(rootJsonc, rootJson);

    if (rootConfig?.compaction) {
        const c = rootConfig.compaction;
        if (c.auto !== undefined || c.prune !== undefined) {
            return { auto: c.auto === true, prune: c.prune === true, resolved: true };
        }
    }

    return { auto: false, prune: false, resolved: false };
}

function readUserCompaction(): { auto: boolean; prune: boolean; resolved: boolean } {
    try {
        const paths = getOpenCodeConfigPaths({ binary: "opencode" });
        const config =
            readJsoncFile<OpenCodeConfig>(paths.configJsonc) ??
            readJsoncFile<OpenCodeConfig>(paths.configJson);

        if (config?.compaction) {
            const c = config.compaction;
            if (c.auto !== undefined || c.prune !== undefined) {
                return { auto: c.auto === true, prune: c.prune === true, resolved: true };
            }
        }
    } catch {}
    return { auto: false, prune: false, resolved: false };
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

/** Raw plugin entries from the effective project-level OpenCode configs under `directory`. */
export function projectPluginEntries(directory: string): Array<string | [string, unknown]> {
    const [dotOcJsonc, dotOcJson, rootJsonc, rootJson] = projectOpenCodeConfigPaths(directory);
    return [
        ...(readEffectiveConfig(dotOcJsonc, dotOcJson)?.plugin ?? []),
        ...(readEffectiveConfig(rootJsonc, rootJson)?.plugin ?? []),
    ];
}

function collectPluginEntries(directory: string): string[] {
    const plugins: string[] = [];

    const pushFrom = (entries: Array<string | [string, unknown]> | undefined) => {
        if (!entries) return;
        for (const entry of entries) {
            const name = extractPluginName(entry);
            if (name) plugins.push(name);
        }
    };

    pushFrom(projectPluginEntries(directory));

    // User-level config
    try {
        const paths = getOpenCodeConfigPaths({ binary: "opencode" });
        pushFrom(readEffectiveConfig(paths.configJsonc, paths.configJson)?.plugin);
    } catch {
        // best-effort
    }

    return plugins;
}

/**
 *
 *
 */
const OMO_PACKAGE_NAMES = new Set(["oh-my-opencode", "oh-my-openagent"]);

/** Whether a project- or user-level OpenCode config lists an OMO plugin entry. */
export function hasOmoPlugin(directory: string): boolean {
    return collectPluginEntries(directory).some((p) => matchesPackageName(p, OMO_PACKAGE_NAMES));
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

    result.preemptiveCompaction = !disabledHooks.has("preemptive-compaction");
    result.contextWindowMonitor = !disabledHooks.has("context-window-monitor");
    result.anthropicRecovery = !disabledHooks.has("anthropic-context-window-limit-recovery");

    return result;
}

function readOmoDisabledHooks(directory: string): Set<string> {
    const disabled = new Set<string>();
    const addAll = (hooks: string[] | undefined) => {
        for (const hook of hooks ?? []) disabled.add(hook);
    };

    const legacyBaseNames = ["oh-my-opencode", "oh-my-openagent"];
    const readLegacy = (dir: string) => {
        for (const base of legacyBaseNames) {
            const config = readEffectiveConfig<OmoConfig>(
                join(dir, `${base}.jsonc`),
                join(dir, `${base}.json`),
            );
            addAll(config?.disabled_hooks);
        }
    };
    const readUnified = (dir: string) => {
        const config = readEffectiveConfig<OmoV2Config>(
            join(dir, "omo.jsonc"),
            join(dir, "omo.json"),
        );
        addAll(config?.["[opencode]"]?.disabled_hooks);
    };

    try {
        readLegacy(getOpenCodeConfigPaths({ binary: "opencode" }).configDir);
    } catch {
        // best-effort
    }
    readLegacy(directory);

    const homeDir = process.env.HOME || homedir();
    readUnified(join(homeDir, ".omo"));
    readUnified(join(directory, ".omo"));

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
