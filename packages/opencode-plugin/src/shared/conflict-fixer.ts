import { existsSync, readFileSync } from "node:fs";
import { parse } from "comment-json";

import { writeFileAtomic } from "./atomic-write";
import {
    asStringArray,
    type ConflictResult,
    DCP_PACKAGE_NAMES,
    extractPluginName,
    matchesPackageName,
    OMO_CONFLICTING_HOOKS,
    omoConfigCandidatePaths,
    openCodeConfigLayerPaths,
} from "./conflict-detector";
import {
    appendJsoncArrayValues,
    isEditableJsonc,
    removeJsoncArrayEntries,
    setJsoncValue,
} from "./jsonc-edit";
import { isRecord } from "./record-type-guard";

type JsonObject = Record<string, unknown>;

interface JsonConfigDocument {
    path: string;
    config: JsonObject;
    text: string;
}

/** A document the editor refuses is skipped so the fixer reports no action for it instead of editing a shadowed value. commentlint: allow(JUDGE) */
function readConfig(filePath: string): JsonConfigDocument | null {
    if (!existsSync(filePath)) {
        return null;
    }

    try {
        const text = readFileSync(filePath, "utf-8");
        const parsed = parse(text);
        return isRecord(parsed) && isEditableJsonc(text)
            ? { path: filePath, config: parsed, text }
            : null;
    } catch {
        return null;
    }
}

/** A truncated `opencode.json` stops OpenCode from starting, so a partial write must never land on the destination path. */
function writeConfig(filePath: string, text: string): void {
    writeFileAtomic(filePath, text);
}

/** Returns parseable layers in the same lowest-to-highest precedence order the host merges them. */
function readOpenCodeLayers(directory: string): JsonConfigDocument[] {
    const layers: JsonConfigDocument[] = [];
    for (const configPath of openCodeConfigLayerPaths(directory)) {
        const document = readConfig(configPath);
        if (document) layers.push(document);
    }
    return layers;
}

type CompactionKey = "auto" | "prune";

/**
 * Picks the layer whose value the host uses for one compaction key: the highest-precedence
 * layer that sets it to a boolean, or the highest-precedence existing layer when no layer
 * sets it and the host default (`auto: true`) is what conflicts. Returns `null` when the
 * winning layer already holds `false` or when no layer exists to edit.
 */
function compactionRepairTarget(
    layers: JsonConfigDocument[],
    key: CompactionKey,
): JsonConfigDocument | null {
    for (let index = layers.length - 1; index >= 0; index -= 1) {
        const layer = layers[index];
        if (!layer) continue;
        const compaction = layer.config.compaction;
        if (isRecord(compaction) && typeof compaction[key] === "boolean") {
            return compaction[key] === false ? null : layer;
        }
    }
    return key === "auto" ? (layers.at(-1) ?? null) : null;
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
    const updatedCompactionKeys = new Set<CompactionKey>();
    let removedDcpPlugin = false;
    let disabledOmoHooks = false;

    const repairCompaction =
        compactionEnabled && (conflicts.compactionAuto || conflicts.compactionPrune);

    if (repairCompaction || conflicts.dcpPlugin) {
        const layers = readOpenCodeLayers(directory);
        // Pending text per path so the compaction and DCP edits to one file compose.
        const pending = new Map<string, string>();

        if (repairCompaction) {
            const keys: CompactionKey[] = [];
            if (conflicts.compactionAuto) keys.push("auto");
            if (conflicts.compactionPrune) keys.push("prune");
            for (const key of keys) {
                const target = compactionRepairTarget(layers, key);
                if (!target) continue;
                const text = pending.get(target.path) ?? target.text;
                if (isRecord(target.config.compaction)) {
                    pending.set(target.path, setJsoncValue(text, ["compaction", key], false));
                    updatedCompactionKeys.add(key);
                } else {
                    // A non-object `compaction` cannot take a nested key; replace the whole block
                    // with every conflicting key set to false.
                    pending.set(
                        target.path,
                        setJsoncValue(
                            text,
                            ["compaction"],
                            Object.fromEntries(keys.map((k) => [k, false])),
                        ),
                    );
                    for (const written of keys) updatedCompactionKeys.add(written);
                }
            }
        }

        if (conflicts.dcpPlugin) {
            for (const layer of layers) {
                const text = pending.get(layer.path) ?? layer.text;
                const result = removeJsoncArrayEntries(text, ["plugin"], (entry) => {
                    const name = extractPluginName(entry);
                    return name ? matchesPackageName(name, DCP_PACKAGE_NAMES) : false;
                });
                if (result.removed) {
                    pending.set(layer.path, result.text);
                    removedDcpPlugin = true;
                }
            }
        }

        for (const [path, text] of pending) {
            writeConfig(path, text);
        }
    }

    const hooksToDisable = (
        Object.entries(OMO_CONFLICTING_HOOKS) as Array<[keyof typeof OMO_CONFLICTING_HOOKS, string]>
    )
        .filter(([conflictKey]) => conflicts[conflictKey])
        .map(([, hook]) => hook);

    if (hooksToDisable.length > 0) {
        for (const candidate of omoConfigCandidatePaths(directory)) {
            const document = readConfig(candidate.path);
            if (!document) {
                continue;
            }

            const block = candidate.unified ? document.config["[opencode]"] : document.config;
            const target = isRecord(block) ? block : {};
            const disabledHooks = new Set(asStringArray(target.disabled_hooks));
            const hooksToAdd = hooksToDisable.filter((hook) => !disabledHooks.has(hook));
            if (hooksToAdd.length === 0) {
                continue;
            }

            const hooksPath = candidate.unified
                ? ["[opencode]", "disabled_hooks"]
                : ["disabled_hooks"];
            // `setJsoncValue` cannot add a child to a primitive or array node.
            const replaceBlock = candidate.unified && block !== undefined && !isRecord(block);
            const text = replaceBlock
                ? setJsoncValue(document.text, ["[opencode]"], { disabled_hooks: hooksToAdd })
                : Array.isArray(target.disabled_hooks)
                  ? appendJsoncArrayValues(document.text, hooksPath, hooksToAdd)
                  : setJsoncValue(document.text, hooksPath, hooksToAdd);
            writeConfig(candidate.path, text);
            disabledOmoHooks = true;
        }
    }

    if (updatedCompactionKeys.has("auto")) {
        actions.push("Disabled auto-compaction");
    }

    if (updatedCompactionKeys.has("prune")) {
        actions.push("Disabled prune");
    }

    if (removedDcpPlugin) {
        actions.push("Removed opencode-dcp plugin");
    }

    if (disabledOmoHooks) {
        actions.push("Disabled conflicting oh-my-opencode hooks");
    }

    return actions;
}
