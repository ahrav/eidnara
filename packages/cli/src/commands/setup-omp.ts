import { ompModelRefToCanonical } from "@eidnara/opencode/shared/harness-provider-map";
import { OmpAdapter } from "../adapters/omp";
import {
    detectOmpBinary,
    getOmpAvailableModels,
    getOmpSetting,
    getOmpVersion,
    listOmpPlugins,
    OMP_PLUGIN_PACKAGE,
    runOmpCommand,
} from "../lib/omp-helpers";
import {
    getOmpAgentDir,
    getOmpNonGlobalConfigSources,
    getOmpPluginsLockPath,
    getSharedUserConfigPath,
} from "../lib/paths";
import {
    type PiCompatibleSetupHost,
    type RunSetupOptions,
    runSetup as runPiCompatibleSetup,
    type SetupEnvironment,
} from "./setup-pi";

const OMP_ENV: SetupEnvironment = {
    detectPiBinary: detectOmpBinary,
    getPiVersion: getOmpVersion,
    getAvailableModels: getOmpAvailableModels,
    paths: {
        getPiAgentConfigDir: getOmpAgentDir,
        getPiUserConfigPath: getSharedUserConfigPath,
        getPiUserExtensionsPath: getOmpPluginsLockPath,
    },
};

const OMP_HOST: PiCompatibleSetupHost = {
    displayName: "Oh My Pi (OMP)",
    cliName: "omp",
    packageSource: OMP_PLUGIN_PACKAGE,
    minimumVersion: "17.1.7",
    versionWarning: (version, minimum) =>
        `OMP ${version} is older than the tested minimum ${minimum}. ` +
        "Upgrade with `omp update` before enabling Eidnara.",
    modelRefToCanonical: ompModelRefToCanonical,
    ensurePluginEntry: async () => new OmpAdapter().ensurePluginEntry(),
    beforeWrite: async ({ binaryPath, cwd, prompts, dryRun, configureHost, eidnara }) => {
        if (!configureHost) {
            const plugins = listOmpPlugins(binaryPath);
            if (plugins === null) {
                prompts.log.error(
                    "Could not list OMP plugins (`omp plugin list --json` failed), so whether Eidnara is already enabled is unknown; refusing to write a shared config that may run beside OMP's native context managers.",
                );
                return false;
            }
            const pluginActive = plugins.some(
                (plugin) => plugin.name === OMP_PLUGIN_PACKAGE && plugin.enabled,
            );
            if (!pluginActive) return async () => {};
        }
        // Project and overlay config decide the effective values `omp config get`
        // reports, so the global values Eidnara will run beside outside this
        // directory are unobservable here. Refuse before any change is planned.
        const nonGlobalSources = getOmpNonGlobalConfigSources(cwd);
        if (nonGlobalSources.length > 0) {
            prompts.log.error(
                "OMP effective settings come from project/overlay config; refusing to mutate the global config or enable Eidnara beside unobserved global settings.\n" +
                    nonGlobalSources.map((path) => `- ${path}`).join("\n") +
                    "\nRun setup from a directory without a project OMP config and with PI_CONFIG_FILES unset.",
            );
            return false;
        }
        const compaction = getOmpSetting(binaryPath, "compaction.enabled");
        const memoryBackend = getOmpSetting(binaryPath, "memory.backend");
        if (compaction === null || memoryBackend === null) {
            prompts.log.error(
                "Could not read OMP compaction/memory settings; refusing to install two context managers blindly.",
            );
            return false;
        }

        const changes: Array<{
            key: "compaction.enabled" | "memory.backend";
            from: string;
            to: string;
        }> = [];
        if (compaction === true && !eidnara.compactionEnabled) {
            prompts.log.info(
                "Eidnara compaction is off in the shared config; leaving OMP native compaction enabled as the context-window owner.",
            );
        } else if (compaction === true) {
            const disable = await prompts.confirm(
                "Disable OMP native compaction? Eidnara must own context management end to end.",
                true,
            );
            if (!disable) {
                prompts.log.error("OMP native compaction conflicts with Eidnara.");
                return false;
            }
            changes.push({ key: "compaction.enabled", from: "true", to: "false" });
        }
        if (memoryBackend !== "off" && !eidnara.memoryEnabled) {
            prompts.log.info(
                `Eidnara memory is off in the shared config; leaving OMP memory backend "${memoryBackend}" enabled.`,
            );
        } else if (memoryBackend !== "off") {
            const disable = await prompts.confirm(
                `Disable OMP memory backend "${memoryBackend}"? Running two automatic memory injectors duplicates context and writes.`,
                true,
            );
            if (!disable) {
                prompts.log.error("OMP automatic memory conflicts with Eidnara memory injection.");
                return false;
            }
            changes.push({ key: "memory.backend", from: memoryBackend, to: "off" });
        }

        if (dryRun) {
            for (const change of changes) {
                prompts.log.message(
                    `[dry-run] would run \`omp config set ${change.key} ${change.to}\``,
                );
            }
            return async () => {};
        }

        const applied: typeof changes = [];
        const rollback = async () => {
            const failed: string[] = [];
            for (const change of [...applied].reverse()) {
                const result = runOmpCommand(
                    binaryPath,
                    ["config", "set", change.key, change.from],
                    10_000,
                );
                if (result.ok) {
                    prompts.log.info(`Restored OMP ${change.key}=${change.from}`);
                } else {
                    const detail = result.stderr ? ` (${result.stderr})` : "";
                    failed.push(`omp config set ${change.key} ${change.from}${detail}`);
                }
            }
            if (failed.length > 0) {
                throw new Error(
                    `Could not restore OMP settings; run by hand:\n${failed.map((step) => `- ${step}`).join("\n")}`,
                );
            }
        };
        for (const change of changes) {
            const result = runOmpCommand(
                binaryPath,
                ["config", "set", change.key, change.to],
                10_000,
            );
            if (!result.ok) {
                prompts.log.error(result.stderr || `Could not set OMP ${change.key}`);
                try {
                    await rollback();
                } catch (rollbackError) {
                    prompts.log.error(
                        rollbackError instanceof Error
                            ? rollbackError.message
                            : String(rollbackError),
                    );
                }
                return false;
            }
            applied.push(change);
            prompts.log.success(`Set OMP ${change.key}=${change.to}`);
        }
        return rollback;
    },
    rollbackPluginEntry: async (registration) => {
        if (registration.action === "already_present") return;
        const action = registration.action === "added" ? "uninstall" : "disable";
        const manualStep = `Run \`omp plugin ${action} ${OMP_PLUGIN_PACKAGE}\` by hand.`;
        const omp = detectOmpBinary();
        if (!omp) {
            throw new Error(
                `Could not ${action} ${OMP_PLUGIN_PACKAGE}: OMP binary not found. ${manualStep}`,
            );
        }
        const result = runOmpCommand(omp.path, ["plugin", action, OMP_PLUGIN_PACKAGE], 120_000);
        if (!result.ok) {
            throw new Error(
                `Could not ${action} ${OMP_PLUGIN_PACKAGE}: ${result.stderr || result.stdout || "omp exited with an error"}. ${manualStep}`,
            );
        }
    },
};

export async function runSetup(options: RunSetupOptions = {}): Promise<number> {
    return runPiCompatibleSetup({
        ...options,
        env: options.env ?? OMP_ENV,
        host: options.host ?? OMP_HOST,
    });
}

export const __test = { OMP_HOST };
