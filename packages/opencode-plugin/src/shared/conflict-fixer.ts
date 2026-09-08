import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import { parse } from "comment-json";

import {
    type ConflictResult,
    DCP_PACKAGE_NAMES,
    extractPluginName,
    matchesPackageName,
    projectOpenCodeConfigPaths,
} from "./conflict-detector";
import { appendJsoncArrayValues, removeJsoncArrayEntries, setJsoncValue } from "./jsonc-edit";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";

type JsonObject = Record<string, unknown>;

const CONFLICTING_OMO_HOOKS = [
    "context-window-monitor",
    "preemptive-compaction",
    "anthropic-context-window-limit-recovery",
] as const;

/** Legacy OMO config base names; each has a `.jsonc`/`.json` pair. The unified layout uses `omo.jsonc`/`omo.json`. */
const OMO_CONFIG_BASE_NAMES = ["oh-my-openagent", "oh-my-opencode"] as const;

function isRecord(value: unknown): value is JsonObject {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asStringArray(value: unknown): string[] {
    return Array.isArray(value)
        ? value.filter((item): item is string => typeof item === "string")
        : [];
}

interface JsonConfigDocument {
    config: JsonObject;
    text: string;
}

function readConfig(filePath: string): JsonConfigDocument | null {
    if (!existsSync(filePath)) {
        return null;
    }

    try {
        const text = readFileSync(filePath, "utf-8");
        const parsed = parse(text);
        return isRecord(parsed) ? { config: parsed, text } : null;
    } catch {
        return null;
    }
}

function writeConfig(filePath: string, text: string): void {
    writeFileSync(filePath, text);
}

function resolveUserOpenCodeConfigPath(): string {
    const paths = getOpenCodeConfigPaths({ binary: "opencode" });
    if (existsSync(paths.configJsonc)) return paths.configJsonc;
    return paths.configJson;
}

/** OpenCode and OMO load one file per directory, `.jsonc` first; the shadowed sibling is not a repair target. */
function effectiveMember(jsoncPath: string, jsonPath: string): string | null {
    if (existsSync(jsoncPath)) return jsoncPath;
    if (existsSync(jsonPath)) return jsonPath;
    return null;
}

function collectOpenCodeConfigPaths(directory: string): string[] {
    const paths = new Set<string>();
    const userConfig = resolveUserOpenCodeConfigPath();

    if (existsSync(userConfig)) {
        paths.add(userConfig);
    }

    const [dotOcJsonc, dotOcJson, rootJsonc, rootJson] = projectOpenCodeConfigPaths(directory);
    for (const filePath of [
        effectiveMember(dotOcJsonc, dotOcJson),
        effectiveMember(rootJsonc, rootJson),
    ]) {
        if (filePath !== null) paths.add(filePath);
    }

    return [...paths];
}

/** Existing OMO config files `fixConflicts` may edit: user and project, legacy and unified layouts. */
export function collectOmoConfigPaths(directory: string): string[] {
    const paths = new Set<string>();
    const configDir = getOpenCodeConfigPaths({ binary: "opencode" }).configDir;
    const add = (path: string | null) => {
        if (path !== null) paths.add(path);
    };

    for (const base of OMO_CONFIG_BASE_NAMES) {
        add(effectiveMember(join(configDir, `${base}.jsonc`), join(configDir, `${base}.json`)));
        add(effectiveMember(join(directory, `${base}.jsonc`), join(directory, `${base}.json`)));
    }

    const homeDir = process.env.HOME || homedir();
    add(effectiveMember(join(homeDir, ".omo", "omo.jsonc"), join(homeDir, ".omo", "omo.json")));
    add(effectiveMember(join(directory, ".omo", "omo.jsonc"), join(directory, ".omo", "omo.json")));

    return [...paths];
}

/* */
function isUnifiedOmoPath(configPath: string): boolean {
    const name = basename(configPath);
    return name === "omo.jsonc" || name === "omo.json";
}

function disableCompactionFlags(
    text: string,
    config: JsonObject,
): { text: string; changed: boolean } {
    if (!isRecord(config.compaction)) {
        return {
            text: setJsoncValue(text, ["compaction"], { auto: false, prune: false }),
            changed: true,
        };
    }

    let updated = text;
    let changed = false;
    if (config.compaction.auto !== false) {
        updated = setJsoncValue(updated, ["compaction", "auto"], false);
        changed = true;
    }
    if (config.compaction.prune !== false) {
        updated = setJsoncValue(updated, ["compaction", "prune"], false);
        changed = true;
    }
    return { text: updated, changed };
}

/**
 *
 * `false` selects compaction-off mode.
 * In compaction-off mode, the fixer does not set `compaction.auto` or `compaction.prune` to `false`.
 *
 */
export interface FixConflictsOptions {
    compactionEnabled?: boolean;
}

export function fixConflicts(
    directory: string,
    conflicts: ConflictResult["conflicts"],
    options?: FixConflictsOptions,
): string[] {
    const compactionEnabled = options?.compactionEnabled ?? true;
    const actions: string[] = [];
    let updatedCompaction = false;
    let removedDcpPlugin = false;
    let disabledOmoHooks = false;

    const repairCompaction =
        compactionEnabled && (conflicts.compactionAuto || conflicts.compactionPrune);

    if (repairCompaction || conflicts.dcpPlugin) {
        for (const configPath of collectOpenCodeConfigPaths(directory)) {
            const document = readConfig(configPath);
            if (!document) {
                continue;
            }

            let text = document.text;
            let changed = false;

            if (repairCompaction) {
                const result = disableCompactionFlags(text, document.config);
                if (result.changed) {
                    text = result.text;
                    changed = true;
                    updatedCompaction = true;
                }
            }

            if (conflicts.dcpPlugin) {
                const result = removeJsoncArrayEntries(text, ["plugin"], (entry) => {
                    const name = extractPluginName(entry);
                    return name ? matchesPackageName(name, DCP_PACKAGE_NAMES) : false;
                });
                if (result.removed) {
                    text = result.text;
                    changed = true;
                    removedDcpPlugin = true;
                }
            }

            if (changed) {
                writeConfig(configPath, text);
            }
        }
    }

    if (
        conflicts.omoContextWindowMonitor ||
        conflicts.omoPreemptiveCompaction ||
        conflicts.omoAnthropicRecovery
    ) {
        const hooksToDisable = new Set<string>();
        if (conflicts.omoContextWindowMonitor) {
            hooksToDisable.add("context-window-monitor");
        }
        if (conflicts.omoPreemptiveCompaction) {
            hooksToDisable.add("preemptive-compaction");
        }
        if (conflicts.omoAnthropicRecovery) {
            hooksToDisable.add("anthropic-context-window-limit-recovery");
        }

        for (const configPath of collectOmoConfigPaths(directory)) {
            const document = readConfig(configPath);
            if (!document) {
                continue;
            }

            const unifiedPath = isUnifiedOmoPath(configPath);
            const target =
                unifiedPath && isRecord(document.config["[opencode]"])
                    ? document.config["[opencode]"]
                    : unifiedPath
                      ? {}
                      : document.config;
            const disabledHooks = new Set(asStringArray(target.disabled_hooks));
            const hooksToAdd = CONFLICTING_OMO_HOOKS.filter(
                (hook) => hooksToDisable.has(hook) && !disabledHooks.has(hook),
            );

            if (hooksToAdd.length > 0) {
                const path = unifiedPath ? ["[opencode]", "disabled_hooks"] : ["disabled_hooks"];
                const text = Array.isArray(target.disabled_hooks)
                    ? appendJsoncArrayValues(document.text, path, hooksToAdd)
                    : setJsoncValue(document.text, path, hooksToAdd);
                writeConfig(configPath, text);
                disabledOmoHooks = true;
            }
        }
    }

    if (updatedCompaction) {
        actions.push("Disabled auto-compaction");
    }

    if (removedDcpPlugin) {
        actions.push("Removed opencode-dcp plugin");
    }

    if (disabledOmoHooks) {
        actions.push("Disabled conflicting oh-my-opencode hooks");
    }

    return actions;
}
