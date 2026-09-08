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

/** An npm install fetches the package and its dependencies, so it gets longer than an enable. */
const INSTALL_TIMEOUT_MS = 300_000;

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
            // `omp plugin install <npm package>` fetches and enables it; the caller's rollback
            // for `added` is `omp plugin uninstall`, which undoes both.
            const install = ["plugin", "install", OMP_PLUGIN_PACKAGE];
            const result = runOmpCommand(omp.path, install, INSTALL_TIMEOUT_MS);
            if (!result.ok) {
                return this.errorResult(
                    configPath,
                    result.stderr || result.stdout || `omp ${install.join(" ")} failed`,
                );
            }
            const after = listOmpPlugins(omp.path);
            if (after?.some((plugin) => plugin.name === OMP_PLUGIN_PACKAGE && plugin.enabled)) {
                return {
                    ok: true,
                    action: "added",
                    message: `Installed ${OMP_PLUGIN_PACKAGE} in OMP.`,
                    configPath,
                };
            }
            // An error result never reaches the caller's `added` rollback, so the install is
            // undone here; otherwise the plugin would stay enabled beside restored native settings.
            const problem =
                after === null
                    ? `could not verify the plugin state after \`omp ${install.join(" ")}\` (\`omp plugin list --json\` failed)`
                    : `${OMP_PLUGIN_PACKAGE} is not enabled after \`omp ${install.join(" ")}\``;
            const uninstall = runOmpCommand(
                omp.path,
                ["plugin", "uninstall", OMP_PLUGIN_PACKAGE],
                INSTALL_TIMEOUT_MS,
            );
            if (!uninstall.ok) {
                return this.errorResult(
                    configPath,
                    `${problem}; removing it again failed (${uninstall.stderr || uninstall.stdout || "omp exited with an error"}). Run \`omp plugin uninstall ${OMP_PLUGIN_PACKAGE}\` by hand if Eidnara must stay off.`,
                    { pluginMayBeActive: true },
                );
            }
            return this.errorResult(configPath, `${problem}; removed it again.`);
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
        const after = listOmpPlugins(omp.path);
        if (after?.some((plugin) => plugin.name === OMP_PLUGIN_PACKAGE && plugin.enabled)) {
            return {
                ok: true,
                action: "updated",
                message: `Enabled ${OMP_PLUGIN_PACKAGE} in OMP.`,
                configPath,
            };
        }
        // `listOmpPlugins` can fail after the command runs, so restore the
        // recorded runtime state rather than infer global state from the
        // project-effective list.
        const problem =
            after === null
                ? `could not verify the plugin state after \`omp ${args.join(" ")}\` (\`omp plugin list --json\` failed)`
                : `${OMP_PLUGIN_PACKAGE} is still disabled in the current project after \`omp ${args.join(" ")}\``;
        if (originalRuntimeEnabled === undefined) {
            return this.errorResult(
                configPath,
                `${problem}; the prior enable state could not be read from ${configPath}, so it was left as is. Check \`omp plugin list\` and run \`omp plugin disable ${OMP_PLUGIN_PACKAGE}\` if Eidnara must stay off.`,
                { pluginMayBeActive: true },
            );
        }
        const restoreAction = originalRuntimeEnabled ? "enable" : "disable";
        const restore = runOmpCommand(
            omp.path,
            ["plugin", restoreAction, OMP_PLUGIN_PACKAGE],
            120_000,
        );
        if (!restore.ok) {
            return this.errorResult(
                configPath,
                `${problem}; restoring the prior plugin state failed (${restore.stderr || restore.stdout || "omp exited with an error"}). Run \`omp plugin ${restoreAction} ${OMP_PLUGIN_PACKAGE}\` by hand.`,
                { pluginMayBeActive: true },
            );
        }
        return this.errorResult(configPath, `${problem}; restored the prior plugin state.`);
    }

    getLogPath(): string {
        return getEidnaraLogPath("pi");
    }

    private readRuntimeEnabled(configPath: string): boolean | undefined {
        return readOmpRuntimeEnabled(configPath);
    }

    private errorResult(
        configPath: string,
        message: string,
        options: { pluginMayBeActive?: boolean } = {},
    ): PluginEntryResult {
        return {
            ok: false,
            action: "error",
            message: `Failed to configure OMP: ${message}`,
            configPath,
            ...(options.pluginMayBeActive ? { pluginMayBeActive: true } : {}),
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
