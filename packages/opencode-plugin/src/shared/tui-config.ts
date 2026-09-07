import { lstatSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { findNodeAtLocation, getNodeValue, type Node } from "jsonc-parser";
import { writeFileAtomicSync } from "./atomic-file";
import { appendJsoncArrayValues, setJsoncValue } from "./jsonc-edit";
import { isJsoncEmpty, parseJsoncTree } from "./jsonc-parser";
import { log } from "./logger";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";
import { resolveWriteTarget } from "./resolve-write-target";

const PLUGIN_NAME = "@eidnara/opencode";
const PLUGIN_ENTRY = `${PLUGIN_NAME}@latest`;

function pluginEntryId(entry: unknown): string {
    if (typeof entry === "string") return entry;
    if (Array.isArray(entry) && typeof entry[0] === "string") return entry[0];
    return "";
}

function isLocalEidnaraDevEntry(entry: unknown): boolean {
    const id = pluginEntryId(entry);
    if (!id) return false;
    if (id === PLUGIN_NAME || id.startsWith(`${PLUGIN_NAME}@`)) return false;
    const isPath =
        id.startsWith("file://") ||
        id.startsWith("/") ||
        id.startsWith("./") ||
        id.startsWith("../") ||
        id.startsWith("~/") ||
        /^[A-Za-z]:[\\/]/.test(id) ||
        id.includes("\\");
    if (!isPath) return false;
    // Whole path components only: `/home/eidnara/other-plugin` is not an Eidnara checkout.
    const components = id
        .replace(/^file:\/\//, "")
        .split(/[\\/]+/)
        .filter(Boolean);
    if (components.includes("opencode-plugin")) return true;
    return components[components.length - 1] === "eidnara";
}

function isEidnaraPluginEntry(entry: unknown): boolean {
    const id = pluginEntryId(entry);
    if (!id) return false;
    if (id === PLUGIN_NAME || id.startsWith(`${PLUGIN_NAME}@`)) return true;
    return isLocalEidnaraDevEntry(entry);
}

/** A dangling `tui.jsonc` symlink is still the user's chosen file, so the entry is tested with `lstat`, not `exists`. */
function entryExists(filePath: string): boolean {
    return lstatSync(filePath, { throwIfNoEntry: false }) !== undefined;
}

function resolveTuiConfigPath(configDirOverride?: string): string {
    const configDir = configDirOverride ?? getOpenCodeConfigPaths({ binary: "opencode" }).configDir;
    const jsoncPath = join(configDir, "tui.jsonc");
    const jsonPath = join(configDir, "tui.json");

    if (entryExists(jsoncPath)) return jsoncPath;
    if (entryExists(jsonPath)) return jsonPath;
    return jsoncPath;
}

/**
 * The edited text, or `null` when the entry is already in place. Edits are
 * applied to the document text so every byte outside the touched entry,
 * including comments and integers beyond `Number.MAX_SAFE_INTEGER`, stays as written.
 */
function withPluginEntry(text: string, tree: Node): string | null {
    const pluginNode = findNodeAtLocation(tree, ["plugin"]);
    const plugins: unknown[] = pluginNode?.type === "array" ? getNodeValue(pluginNode) : [];

    const existingIdx = plugins.findIndex(isEidnaraPluginEntry);
    if (existingIdx >= 0) {
        const existing = plugins[existingIdx];
        if (isLocalEidnaraDevEntry(existing)) return null;
        const id = pluginEntryId(existing);
        if (id !== PLUGIN_NAME) return null;
        const location = Array.isArray(existing)
            ? ["plugin", existingIdx, 0]
            : ["plugin", existingIdx];
        return setJsoncValue(text, location, PLUGIN_ENTRY);
    }
    if (pluginNode?.type === "array") {
        return appendJsoncArrayValues(text, ["plugin"], [PLUGIN_ENTRY]);
    }
    return setJsoncValue(text, ["plugin"], [PLUGIN_ENTRY]);
}

export function ensureTuiPluginEntry(options: { configDir?: string } = {}): boolean {
    try {
        const configPath = resolveTuiConfigPath(options.configDir);
        // One resolution serves the read, the staging file, and the rename, so a
        // link retargeted mid-write cannot receive the previous target's snapshot.
        const target = resolveWriteTarget(configPath);

        let raw = "";
        if (lstatSync(target, { throwIfNoEntry: false })?.isFile()) {
            raw = readFileSync(target, "utf-8");
        }
        // `setJsoncValue` requires a value node; appending `{}` preserves comments preceding the new entry.
        const text = isJsoncEmpty(raw) ? `${raw}\n{}` : raw;
        const tree = parseJsoncTree(text);
        if (tree.type !== "object") {
            // Replacing an array, scalar, or null root would discard the user's document.
            log(`[eidnara] ${configPath} has a non-object root; leaving it unchanged`);
            return false;
        }

        const next = withPluginEntry(text, tree);
        if (next === null) return false;

        mkdirSync(dirname(target), { recursive: true });
        writeFileAtomicSync(target, next.endsWith("\n") ? next : `${next}\n`);
        log(`[eidnara] updated TUI plugin entry in ${configPath}`);
        return true;
    } catch (error) {
        log(
            `[eidnara] failed to update tui.json: ${error instanceof Error ? error.message : String(error)}`,
        );
        return false;
    }
}
