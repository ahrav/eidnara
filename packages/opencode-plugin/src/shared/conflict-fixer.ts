import { realpathSync } from "node:fs";

import { writeFileAtomicSync } from "./atomic-file";
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
import { parseConfigJsonc, readJsoncBytes } from "./jsonc-parser";
import { isRecord } from "./record-type-guard";

type JsonObject = Record<string, unknown>;

interface JsonConfigDocument {
    path: string;
    /** The physical file behind `path` after every symlink, in the path and in its ancestors, is resolved; aliases of one file compose here. */
    target: string;
    config: JsonObject;
    text: string;
    /** `false` when the editor refuses the document; the layer still counts toward precedence. */
    editable: boolean;
}

/**
 * `config` is parsed the way the detector parses it, so a layer the editor refuses still reports the value the host uses.
 * The resolved target serves both read and write, preventing a retargeted link from receiving the previous target's edited snapshot.
 * A FIFO or device is refused before the read so a blocking open cannot hang the fixer.
 * A fatal decoder rejects malformed UTF-8 rather than rewriting the file with U+FFFD.
 */
function readConfig(filePath: string): JsonConfigDocument | null {
    try {
        const target = realpathSync(filePath);
        const text = readJsoncBytes(target);
        const parsed = parseConfigJsonc<unknown>(text);
        if (!isRecord(parsed)) return null;
        return {
            path: filePath,
            target,
            config: parsed,
            text,
            editable: isEditableJsonc(text),
        };
    } catch {
        return null;
    }
}

/** A truncated `opencode.json` stops OpenCode from starting, so a partial write must never land on the destination path. */
function writeConfig(target: string, text: string): void {
    writeFileAtomicSync(target, text);
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

export function collectOmoConfigPaths(directory: string): string[] {
    return omoConfigCandidatePaths(directory).map((candidate) => candidate.path);
}

type CompactionKey = "auto" | "prune";

const HOST_COMPACTION_DEFAULTS: Record<CompactionKey, boolean> = { auto: true, prune: false };

/**
 * Picks the layer whose value the host uses for one compaction key: the highest-precedence
 * layer that sets it to a boolean, or the highest-precedence editable layer when no layer sets
 * it and the host default differs from `value`. Returns `null` when the winning layer already
 * holds `value` or when a read-only winning layer takes precedence over editable layers. A
 * repair target requires an editable layer.
 */
function compactionRepairTarget(
    layers: JsonConfigDocument[],
    key: CompactionKey,
    value: boolean,
): JsonConfigDocument | null {
    for (let index = layers.length - 1; index >= 0; index -= 1) {
        const layer = layers[index];
        if (!layer) continue;
        const compaction = layer.config.compaction;
        if (isRecord(compaction) && typeof compaction[key] === "boolean") {
            return compaction[key] === value || !layer.editable ? null : layer;
        }
    }
    if (HOST_COMPACTION_DEFAULTS[key] === value) return null;
    return layers.filter((layer) => layer.editable).at(-1) ?? null;
}

/**
 * Uneditable layers can leave conflicts unresolved; callers re-run `detectConflicts` to report
 * remaining conflicts.
 */
export function fixConflicts(
    directory: string,
    repair: Pick<ConflictResult, "conflicts" | "compactionPatch">,
): string[] {
    const { conflicts, compactionPatch } = repair;
    const actions: string[] = [];
    const updatedCompactionKeys = new Map<CompactionKey, boolean>();
    let removedDcpPlugin = false;
    let disabledOmoHooks = false;

    const patchKeys = (["auto", "prune"] as const).filter(
        (key) => compactionPatch[key] !== undefined,
    );

    if (patchKeys.length > 0 || conflicts.dcpPlugin) {
        const layers = readOpenCodeLayers(directory);
        // Pending text per resolved target so the compaction and DCP edits to one file compose,
        // including when two layer paths are symlinks to the same file.
        const pending = new Map<string, string>();

        for (const key of patchKeys) {
            const value = compactionPatch[key] as boolean;
            const target = compactionRepairTarget(layers, key, value);
            if (!target) continue;
            const text = pending.get(target.target) ?? target.text;
            if (isRecord(target.config.compaction)) {
                pending.set(target.target, setJsoncValue(text, ["compaction", key], value));
                updatedCompactionKeys.set(key, value);
            } else {
                // A non-object `compaction` requires whole-block replacement to apply the patched keys.
                pending.set(target.target, setJsoncValue(text, ["compaction"], compactionPatch));
                for (const written of patchKeys) {
                    updatedCompactionKeys.set(written, compactionPatch[written] as boolean);
                }
            }
        }

        if (conflicts.dcpPlugin) {
            for (const layer of layers) {
                if (!layer.editable) continue;
                const text = pending.get(layer.target) ?? layer.text;
                const result = removeJsoncArrayEntries(text, ["plugin"], (entry) => {
                    const name = extractPluginName(entry);
                    return name ? matchesPackageName(name, DCP_PACKAGE_NAMES) : false;
                });
                if (result.removed) {
                    pending.set(layer.target, result.text);
                    removedDcpPlugin = true;
                }
            }
        }

        for (const [target, text] of pending) {
            writeConfig(target, text);
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
            if (!document || !document.editable) {
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
            writeConfig(document.target, text);
            disabledOmoHooks = true;
        }
    }

    const auto = updatedCompactionKeys.get("auto");
    if (auto !== undefined) {
        actions.push(auto ? "Enabled auto-compaction" : "Disabled auto-compaction");
    }

    const prune = updatedCompactionKeys.get("prune");
    if (prune !== undefined) {
        actions.push(prune ? "Enabled prune" : "Disabled prune");
    }

    if (removedDcpPlugin) {
        actions.push("Removed opencode-dcp plugin");
    }

    if (disabledOmoHooks) {
        actions.push("Disabled conflicting oh-my-opencode hooks");
    }

    return actions;
}
