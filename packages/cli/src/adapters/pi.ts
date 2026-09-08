import { stringify as stringifyJsonc } from "comment-json";
import { writeFileAtomic } from "../lib/atomic-write";
import { ensureParentDir } from "../lib/fs-utils";
import { readJsoncConfig, readJsoncConfigForUpdate } from "../lib/jsonc-config";
import {
    getEidnaraLogPath,
    getPiAgentDir,
    getPiUserExtensionsPath,
    getSharedUserConfigPath,
} from "../lib/paths";
import { detectPiBinary, PI_PACKAGE_SOURCE } from "../lib/pi-helpers";
import type { HarnessAdapter, HarnessConfigPaths, PluginEntryResult } from "./types";

const PLUGIN_NAME = "@eidnara/pi";
const SETTINGS_BASENAME = "settings.json";

export class PiAdapter implements HarnessAdapter {
    readonly kind = "pi" as const;
    readonly displayName = "Pi";
    readonly pluginPackageName = PLUGIN_NAME;

    isInstalled(): boolean {
        return detectPiBinary() !== null;
    }

    hasPluginEntry(): boolean {
        const settings = readPiSettings();
        if (!settings) return false;
        const packages = Array.isArray(settings.packages) ? settings.packages : [];
        return packages.some((entry) => entry === PI_PACKAGE_SOURCE);
    }

    getConfigPaths(): HarnessConfigPaths {
        const dir = getPiAgentDir();
        return {
            configDir: dir,
            pluginConfigPath: getPiUserExtensionsPath(),
            eidnaraConfigPath: getSharedUserConfigPath(),
            secondaryConfigPath: null,
        };
    }

    async ensurePluginEntry(): Promise<PluginEntryResult> {
        const settingsPath = getPiUserExtensionsPath();
        try {
            const settings = readPiSettingsForUpdate();
            const packages = Array.isArray(settings.packages) ? settings.packages : [];

            const idx = packages.indexOf(PI_PACKAGE_SOURCE);
            if (idx === -1) {
                packages.push(PI_PACKAGE_SOURCE);
                settings.packages = packages;
                writePiSettings(settings);
                return {
                    ok: true,
                    action: "added",
                    message: `Added ${PI_PACKAGE_SOURCE} to ${settingsPath}.`,
                    configPath: settingsPath,
                };
            }
            return {
                ok: true,
                action: "already_present",
                message: `Plugin entry already present in ${settingsPath}.`,
                configPath: settingsPath,
            };
        } catch (err) {
            return {
                ok: false,
                action: "error",
                message: `Failed to update ${settingsPath}: ${(err as Error).message}`,
                configPath: settingsPath,
            };
        }
    }

    getLogPath(): string {
        return getEidnaraLogPath("pi");
    }
}

interface PiSettingsLike {
    packages?: unknown[];
    [k: string]: unknown;
}

function readPiSettings(): PiSettingsLike | null {
    const result = readJsoncConfig(getPiUserExtensionsPath());
    return result.kind === "parsed" ? (result.value as PiSettingsLike) : null;
}

function readPiSettingsForUpdate(): PiSettingsLike {
    return readJsoncConfigForUpdate(getPiUserExtensionsPath()) as PiSettingsLike;
}

function writePiSettings(settings: PiSettingsLike): void {
    const settingsPath = getPiUserExtensionsPath();
    ensureParentDir(settingsPath);
    const text = stringifyJsonc(settings, null, 2);
    writeFileAtomic(settingsPath, `${text}\n`);
}

// SETTINGS_BASENAME is exported for tests that need it.
export { SETTINGS_BASENAME };
