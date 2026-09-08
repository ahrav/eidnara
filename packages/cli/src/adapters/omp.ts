import { readRegularFileSync } from "@eidnara/opencode/shared/regular-file";
import {
    detectOmpBinary,
    listOmpPlugins,
    OMP_PLUGIN_PACKAGE,
    runOmpCommand,
} from "../lib/omp-helpers";
import {
    getEidnaraLogPath,
    getOmpAgentDir,
    getOmpPluginsLockPath,
    getSharedUserConfigPath,
} from "../lib/paths";
import type { HarnessAdapter, HarnessConfigPaths, PluginEntryResult } from "./types";

export class OmpAdapter implements HarnessAdapter {
    readonly kind = "omp" as const;
    readonly displayName = "Oh My Pi (OMP)";
    readonly pluginPackageName = OMP_PLUGIN_PACKAGE;

    isInstalled(): boolean {
        return detectOmpBinary() !== null;
    }

    hasPluginEntry(): boolean {
        const omp = detectOmpBinary();
        if (!omp) return false;
        return (
            listOmpPlugins(omp.path)?.some(
                (plugin) => plugin.name === OMP_PLUGIN_PACKAGE && plugin.enabled,
            ) ?? false
        );
    }

    getConfigPaths(): HarnessConfigPaths {
        return {
            configDir: getOmpAgentDir(),
            pluginConfigPath: getOmpPluginsLockPath(),
            eidnaraConfigPath: getSharedUserConfigPath(),
            secondaryConfigPath: null,
        };
    }

    async ensurePluginEntry(): Promise<PluginEntryResult> {
        const configPath = getOmpPluginsLockPath();
        const omp = detectOmpBinary();
        if (!omp) return this.errorResult(configPath, "OMP binary not found");
        const plugins = listOmpPlugins(omp.path);
        if (plugins === null) {
            return this.errorResult(configPath, "`omp plugin list --json` failed");
        }
        const installed = plugins.find((plugin) => plugin.name === OMP_PLUGIN_PACKAGE);
        if (installed?.enabled) {
            return {
                ok: true,
                action: "already_present",
                message: `${OMP_PLUGIN_PACKAGE} is already enabled in OMP.`,
                configPath,
            };
        }
        if (!installed) {
            return this.errorResult(configPath, `${OMP_PLUGIN_PACKAGE} is not installed in OMP`);
        }
        const originalRuntimeEnabled = this.readRuntimeEnabled(configPath);

        const args = ["plugin", "enable", OMP_PLUGIN_PACKAGE];
        const result = runOmpCommand(omp.path, args, 120_000);
        if (!result.ok) {
            return this.errorResult(
                configPath,
                result.stderr || result.stdout || `omp ${args.join(" ")} failed`,
            );
        }
        const enabledAfter = listOmpPlugins(omp.path)?.some(
            (plugin) => plugin.name === OMP_PLUGIN_PACKAGE && plugin.enabled,
        );
        if (!enabledAfter) {
            // A project override can keep the plugin disabled despite a zero exit status.
            // Recovery restores the prior runtime enable state when it is known.
            // The recovery path must not infer global state from the project-effective plugin list.
            if (originalRuntimeEnabled !== undefined) {
                runOmpCommand(
                    omp.path,
                    ["plugin", originalRuntimeEnabled ? "enable" : "disable", OMP_PLUGIN_PACKAGE],
                    120_000,
                );
            }
            return this.errorResult(
                configPath,
                `${OMP_PLUGIN_PACKAGE} is still disabled in the current project after \`omp ${args.join(" ")}\``,
            );
        }
        return {
            ok: true,
            action: "updated",
            message: `Enabled ${OMP_PLUGIN_PACKAGE} in OMP.`,
            configPath,
        };
    }

    getLogPath(): string {
        return getEidnaraLogPath("pi");
    }

    private readRuntimeEnabled(configPath: string): boolean | undefined {
        return readOmpRuntimeEnabled(configPath);
    }

    private errorResult(configPath: string, message: string): PluginEntryResult {
        return {
            ok: false,
            action: "error",
            message: `Failed to configure OMP: ${message}`,
            configPath,
        };
    }
}

/** The prior enable state from the plugins lock, or undefined when the lock is unreadable, non-regular, or silent. */
export function readOmpRuntimeEnabled(lockPath: string): boolean | undefined {
    try {
        const lock = JSON.parse(readRegularFileSync(lockPath)) as {
            plugins?: Record<string, { enabled?: unknown }>;
        };
        const enabled = lock.plugins?.[OMP_PLUGIN_PACKAGE]?.enabled;
        return typeof enabled === "boolean" ? enabled : undefined;
    } catch {
        return undefined;
    }
}
