/**
 *
 */

import {
    chmodSync,
    existsSync,
    mkdirSync,
    readFileSync,
    realpathSync,
    renameSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { parse, stringify } from "comment-json";
import { stripJsonComments } from "./jsonc-parser";
import { log } from "./logger";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";
import { isRecord } from "./record-type-guard";

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
        id.includes("\\");
    if (!isPath) return false;
    return id.includes("opencode-plugin") || id.includes("eidnara");
}

function isEidnaraPluginEntry(entry: unknown): boolean {
    const id = pluginEntryId(entry);
    if (!id) return false;
    if (id === PLUGIN_NAME || id.startsWith(`${PLUGIN_NAME}@`)) return true;
    return isLocalEidnaraDevEntry(entry);
}

function resolveWriteTarget(configPath: string): string {
    try {
        return realpathSync(configPath);
    } catch {
        return configPath;
    }
}

function writeTuiConfigAtomic(configPath: string, config: Record<string, unknown>): void {
    const body = `${stringify(config, null, 2)}\n`;
    // Resolve symlinks so rename updates the linked file instead of replacing the link.
    const target = resolveWriteTarget(configPath);
    // A per-process staging name keeps two concurrent writers from publishing
    // each other's partially written file through the shared rename target.
    const tmpPath = `${target}.${process.pid}.tmp`;
    writeFileSync(tmpPath, body);
    try {
        if (statSync(target, { throwIfNoEntry: false })?.isFile()) {
            chmodSync(tmpPath, statSync(target).mode & 0o777);
        }
    } catch {
        /* new file */
    }
    renameSync(tmpPath, target);
}

function resolveTuiConfigPath(configDirOverride?: string): string {
    const configDir = configDirOverride ?? getOpenCodeConfigPaths({ binary: "opencode" }).configDir;
    const jsoncPath = join(configDir, "tui.jsonc");
    const jsonPath = join(configDir, "tui.json");

    if (existsSync(jsoncPath)) return jsoncPath;
    if (existsSync(jsonPath)) return jsonPath;
    return jsoncPath;
}

export function ensureTuiPluginEntry(options: { configDir?: string } = {}): boolean {
    try {
        const configPath = resolveTuiConfigPath(options.configDir);

        let config: Record<string, unknown> = {};
        if (existsSync(configPath)) {
            const raw = readFileSync(configPath, "utf-8");
            // comment-json rejects input with no JSON value, so an empty or
            // comment-only file counts as an empty config, not a parse failure.
            const parsed: unknown = stripJsonComments(raw).trim() === "" ? {} : parse(raw);
            if (isRecord(parsed)) config = parsed;
        }

        // The parsed array is mutated in place: comment-json attaches comments
        // to the array object as symbol properties, and a spread copy drops them.
        let plugins: unknown[];
        if (Array.isArray(config.plugin)) {
            plugins = config.plugin;
        } else {
            plugins = [];
            config.plugin = plugins;
        }

        const existingIdx = plugins.findIndex(isEidnaraPluginEntry);
        if (existingIdx >= 0) {
            const existing = plugins[existingIdx];
            if (isLocalEidnaraDevEntry(existing)) {
                return false;
            }
            const id = pluginEntryId(existing);
            if (id === PLUGIN_ENTRY) {
                return false;
            }
            if (id !== PLUGIN_NAME) {
                return false;
            }
            if (Array.isArray(existing) && existing.length >= 1) {
                existing[0] = PLUGIN_ENTRY;
            } else {
                plugins[existingIdx] = PLUGIN_ENTRY;
            }
        } else {
            plugins.push(PLUGIN_ENTRY);
        }

        mkdirSync(dirname(configPath), { recursive: true });
        writeTuiConfigAtomic(configPath, config);
        log(`[eidnara] updated TUI plugin entry in ${configPath}`);
        return true;
    } catch (error) {
        log(
            `[eidnara] failed to update tui.json: ${error instanceof Error ? error.message : String(error)}`,
        );
        return false;
    }
}
