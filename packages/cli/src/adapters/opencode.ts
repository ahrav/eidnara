import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, parse as parsePath, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { stringify as stringifyJsonc } from "comment-json";
import { resolveLinkTarget, writeFileAtomic } from "../lib/atomic-write";
import { ensureParentDir } from "../lib/fs-utils";
import { readJsoncConfig, readJsoncConfigForUpdate } from "../lib/jsonc-config";
import { detectOpenCode } from "../lib/opencode-detect";
import { detectConfigPaths, envFirstHomeDir, getEidnaraLogPath } from "../lib/paths";
import type { HarnessAdapter, HarnessConfigPaths, PluginEntryResult } from "./types";

const PLUGIN_NAME = "@eidnara/opencode";

export class OpenCodeAdapter implements HarnessAdapter {
    readonly kind = "opencode" as const;
    readonly displayName = "OpenCode";
    readonly pluginPackageName = PLUGIN_NAME;

    isInstalled(): boolean {
        // A Desktop-only install (no CLI on PATH) still counts as installed:
        // OpenCode Desktop ships no invocable `opencode` binary, so a binary
        // check alone would wrongly report OpenCode as absent.
        return detectOpenCode().kind !== "none";
    }

    hasPluginEntry(): boolean {
        const paths = detectConfigPaths();
        if (paths.opencodeConfigFormat === "none") return false;
        const result = readJsoncConfig(paths.opencodeConfig);
        if (result.kind !== "parsed") return false;
        const plugin = result.value.plugin;
        if (!Array.isArray(plugin)) return false;
        return plugin.some(
            (entry) => matchesPluginEntry(entry, PLUGIN_NAME) || isDevPathPluginEntry(entry),
        );
    }

    getConfigPaths(): HarnessConfigPaths {
        const paths = detectConfigPaths();
        return {
            configDir: paths.configDir,
            pluginConfigPath: paths.opencodeConfig,
            eidnaraConfigPath: paths.eidnaraConfig,
            secondaryConfigPath: paths.tuiConfig,
        };
    }

    async ensurePluginEntry(): Promise<PluginEntryResult> {
        const paths = detectConfigPaths();
        const target = paths.opencodeConfig;
        try {
            const file = resolveLinkTarget(target);
            const exists = paths.opencodeConfigFormat !== "none";
            if (!exists) {
                const initial = {
                    $schema: "https://opencode.ai/config.json",
                    plugin: [PLUGIN_NAME],
                };
                ensureParentDir(target);
                writeFileAtomic(file, `${JSON.stringify(initial, null, 4)}\n`);
                return {
                    ok: true,
                    action: "added",
                    message: `Created ${target} with plugin entry.`,
                    configPath: target,
                };
            }

            const cfg = readJsoncConfigForUpdate(file);

            const plugin = Array.isArray(cfg.plugin) ? cfg.plugin : [];
            const existingIdx = plugin.findIndex((e) => matchesPluginEntry(e, PLUGIN_NAME));
            const existingDevIdx = plugin.findIndex((e) => isDevPathPluginEntry(e));

            // Setup preserves local dev-path entries instead of adding or replacing them.
            if (existingIdx === -1 && existingDevIdx === -1) {
                plugin.push(PLUGIN_NAME);
                cfg.plugin = plugin;
                writeFileAtomic(file, `${stringifyJsonc(cfg, null, 4)}\n`);
                return {
                    ok: true,
                    action: "added",
                    message: `Added ${PLUGIN_NAME} to ${target}.`,
                    configPath: target,
                };
            }

            if (existingDevIdx !== -1) {
                const devEntry = String(plugin[existingDevIdx]);
                return {
                    ok: true,
                    action: "already_present",
                    message: `Plugin already present (dev path: ${devEntry}) in ${target}.`,
                    configPath: target,
                };
            }

            return {
                ok: true,
                action: "already_present",
                message: `Plugin entry already present in ${target}.`,
                configPath: target,
            };
        } catch (err) {
            return {
                ok: false,
                action: "error",
                message: `Failed to update ${target}: ${(err as Error).message}`,
                configPath: target,
            };
        }
    }

    getLogPath(): string {
        return getEidnaraLogPath("opencode");
    }
}

export function isLocalPathPluginEntry(entry: unknown): boolean {
    const candidate =
        typeof entry === "string"
            ? entry
            : Array.isArray(entry) && typeof entry[0] === "string"
              ? entry[0]
              : null;
    if (!candidate) return false;
    // Windows configs spell relative entries with backslashes, which `isAbsolute` does not cover.
    return (
        candidate.startsWith("file://") ||
        isAbsolute(candidate) ||
        /^\.\.?[\\/]/.test(candidate) ||
        /^~[\\/]/.test(candidate)
    );
}

/** OpenCode expands a leading `~` in a plugin path to the home directory. */
function expandHomePrefix(path: string): string {
    return /^~[\\/]/.test(path) ? join(envFirstHomeDir(), path.slice(2)) : path;
}

/**
 * Match a local plugin entry only when its nearest package.json identifies the
 * OpenCode Eidnara package. A basename substring is not sufficient: paths
 * such as `eidnara-theme` must not suppress the real plugin registration.
 */
export function isDevPathPluginEntry(entry: unknown, baseDir: string = process.cwd()): boolean {
    const candidate =
        typeof entry === "string"
            ? entry
            : Array.isArray(entry) && typeof entry[0] === "string"
              ? entry[0]
              : null;
    if (!candidate || !isLocalPathPluginEntry(entry)) return false;

    let localPath: string;
    try {
        if (candidate.startsWith("file://")) {
            localPath = fileURLToPath(candidate);
        } else {
            // A `~` entry is home-relative; any other relative entry is relative to the project
            // whose config declares it.
            localPath = resolve(baseDir, expandHomePrefix(candidate));
        }

        if (statSync(localPath).isFile()) localPath = dirname(localPath);
        const root = parsePath(localPath).root;
        while (localPath !== root) {
            const packagePath = resolve(localPath, "package.json");
            if (existsSync(packagePath)) {
                const pkg = JSON.parse(readFileSync(packagePath, "utf8")) as { name?: unknown };
                return pkg.name === PLUGIN_NAME;
            }
            localPath = dirname(localPath);
        }
    } catch {
        // An unreadable or unresolved path cannot prove that our plugin is installed.
    }
    return false;
}

/**
 * Returns false for `file://` entries so dev paths are not classified as
 * "the published plugin". Use `isDevPathPluginEntry` for that detection.
 */
export function matchesPluginEntry(entry: unknown, pkgName: string): boolean {
    let candidate: string | null = null;
    if (typeof entry === "string") candidate = entry;
    else if (Array.isArray(entry) && typeof entry[0] === "string") candidate = entry[0];
    if (!candidate) return false;
    if (candidate.startsWith("file://")) return false;
    // The matcher uses the final `@` so scoped package names retain their scope.
    const at = candidate.lastIndexOf("@");
    const head = at > 0 ? candidate.slice(0, at) : candidate;
    return head === pkgName;
}
