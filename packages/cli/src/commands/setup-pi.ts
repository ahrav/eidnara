import { existsSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import { resolveEidnaraProjectConfigPath } from "@eidnara/opencode/config/config-paths";
import { piModelRefToCanonical } from "@eidnara/opencode/shared/harness-provider-map";
import { isRecord } from "@eidnara/opencode/shared/record-type-guard";
import { stringify as stringifyJsonc } from "comment-json";
import type { PluginEntryResult } from "../adapters/types";
import { writeFileAtomic } from "../lib/atomic-write";
import {
    assertJsoncConfigsParseable,
    readJsoncConfigForUpdate,
    readJsoncLenient,
} from "../lib/jsonc-config";
import { pickModel } from "../lib/model-picker";
import { getPiAgentDir, getPiUserExtensionsPath, getSharedUserConfigPath } from "../lib/paths";
import {
    detectPiBinary,
    getAvailableModels,
    getPiVersion,
    PI_PACKAGE_SOURCE,
} from "../lib/pi-helpers";
import type { PromptIO } from "../lib/prompts";

export interface SetupEnvironment {
    detectPiBinary: () => { path: string } | null;
    getPiVersion: typeof getPiVersion;
    getAvailableModels: typeof getAvailableModels;
    paths: {
        getPiAgentConfigDir: typeof getPiAgentDir;
        getPiUserConfigPath: typeof getSharedUserConfigPath;
        getPiUserExtensionsPath: typeof getPiUserExtensionsPath;
    };
}

/** Throw when a restoration did not take effect so the caller reports a partial rollback. */
export type SetupRollback = () => Promise<void>;

/** Shared-config modes a host hook reads before disabling a native manager. */
export interface EidnaraModes {
    enabled: boolean;
    /** False when Eidnara compaction is off or Eidnara is disabled. */
    compactionEnabled: boolean;
    /** False when Eidnara memory is off or Eidnara is disabled. */
    memoryEnabled: boolean;
}

export interface PiCompatibleSetupHost {
    displayName: string;
    cliName: string;
    packageSource: string;
    minimumVersion?: string;
    versionWarning?: (version: string, minimum: string) => string;
    /** Convert this host's model selector to the shared canonical config form. */
    modelRefToCanonical?: (ref: string) => string;
    ensurePluginEntry: (settingsPath: string) => Promise<PluginEntryResult>;
    beforeWrite?: (options: {
        binaryPath: string;
        cwd: string;
        prompts: PromptIO;
        dryRun: boolean;
        configureHost: boolean;
        eidnara: EidnaraModes;
    }) => Promise<SetupRollback | false>;
    /**
     * Undo a successful `ensurePluginEntry`. Throw when the undo did not take
     * effect: the caller then skips the native-settings rollback, because
     * restoring native context managers while the plugin stays registered
     * runs two managers side by side.
     */
    rollbackPluginEntry?: (registration: PluginEntryResult) => Promise<void>;
}

export interface RunSetupOptions {
    prompts?: PromptIO;
    env?: SetupEnvironment;
    host?: PiCompatibleSetupHost;
    /**
     * When true, run the interactive wizard without writing files or registering plugins; print planned changes instead.
     */
    dryRun?: boolean;
}

const DEFAULT_ENV: SetupEnvironment = {
    detectPiBinary,
    getPiVersion,
    getAvailableModels,
    paths: {
        getPiAgentConfigDir: getPiAgentDir,
        getPiUserConfigPath: getSharedUserConfigPath,
        getPiUserExtensionsPath,
    },
};

const DEFAULT_HOST: PiCompatibleSetupHost = {
    displayName: "Pi",
    cliName: "pi",
    packageSource: PI_PACKAGE_SOURCE,
    minimumVersion: "0.74.0",
    versionWarning: (version, minimum) =>
        `Pi ${version} is older than the required ${minimum}.\n` +
        `Pi 0.74.0 renamed the npm package from \`@mariozechner/pi-coding-agent\` ` +
        `to \`@earendil-works/pi-coding-agent\`. Eidnara's peer dependency ` +
        `targets the new scope, so older Pi installs cannot load this extension.\n` +
        `Run \`pi update --self\` (or \`npm install -g @earendil-works/pi-coding-agent@latest\`) before continuing.`,
    ensurePluginEntry: async (settingsPath) => {
        const settings = readJsoncConfigForUpdate(settingsPath);
        const packagesFieldExisted = Object.hasOwn(settings, "packages");
        try {
            const added = writePiSettingsPackage(settingsPath);
            return {
                ok: true,
                action: added ? "added" : "already_present",
                message: added
                    ? `Added ${PI_PACKAGE_SOURCE} to ${settingsPath}`
                    : `Eidnara package already present in ${settingsPath}`,
                configPath: settingsPath,
                packagesFieldExisted,
            };
        } catch (error) {
            return {
                ok: false,
                action: "error",
                message: error instanceof Error ? error.message : String(error),
                configPath: settingsPath,
            };
        }
    },
    rollbackPluginEntry: async (registration) => {
        if (registration.action === "added") {
            removePiSettingsPackage(
                registration.configPath,
                PI_PACKAGE_SOURCE,
                (registration as PluginEntryResult & { packagesFieldExisted?: boolean })
                    .packagesFieldExisted === false,
            );
        }
    },
};

function ensureDir(path: string): void {
    if (!existsSync(path)) mkdirSync(path, { recursive: true });
}

async function getDefaultPrompts(): Promise<PromptIO> {
    const { promptIO } = await import("../lib/prompts");
    return promptIO;
}

function compactObject<T extends Record<string, unknown>>(obj: T): T {
    for (const key of Object.keys(obj)) {
        if (obj[key] === undefined) delete obj[key];
    }
    return obj;
}

/**
 * The comparison parses X.Y.Z versions and ignores pre-release and build suffixes.
 * Returns -1 if `a < b`, 0 if equal, and 1 if `a > b`.
 * either string can't be parsed (we conservatively assume "good enough" so
 * a parse failure doesn't block the user with a phantom upgrade prompt).
 */
function comparePiVersion(a: string, b: string): number {
    const parse = (v: string): [number, number, number] | null => {
        const match = v.match(/(\d+)\.(\d+)\.(\d+)/);
        return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : null;
    };
    const left = parse(a);
    const right = parse(b);
    if (!left || !right) return 0;
    for (let i = 0; i < 3; i += 1) {
        if (left[i] < right[i]) return -1;
        if (left[i] > right[i]) return 1;
    }
    return 0;
}

export function writePiSettingsPackage(
    settingsPath: string,
    packageSource = PI_PACKAGE_SOURCE,
): boolean {
    const settings = readJsoncConfigForUpdate(settingsPath);
    if (settings.packages !== undefined && !Array.isArray(settings.packages)) {
        // Refuse to replace a non-array `packages` value; overwriting it would discard user configuration.
        throw new Error(
            `Refusing to rewrite ${settingsPath}: "packages" is ${typeof settings.packages}, expected an array. Fix it by hand, then rerun setup.`,
        );
    }
    const packages = Array.isArray(settings.packages) ? settings.packages : [];
    if (packages.some((entry) => entry === packageSource)) return false;

    packages.push(packageSource);
    settings.packages = packages;
    writeFileAtomic(settingsPath, `${stringifyJsonc(settings, null, 2)}\n`);
    return true;
}
export function removePiSettingsPackage(
    settingsPath: string,
    packageSource = PI_PACKAGE_SOURCE,
    removeFieldWhenEmpty = false,
): boolean {
    const settings = readJsoncConfigForUpdate(settingsPath);
    if (!Array.isArray(settings.packages)) return false;
    const packages = settings.packages;
    // Splicing in place preserves comments that comment-json attaches to
    // remaining entries; `filter` drops them.
    let removed = false;
    for (let index = packages.length - 1; index >= 0; index -= 1) {
        if (packages[index] !== packageSource) continue;
        packages.splice(index, 1);
        removed = true;
    }
    if (!removed) return false;
    if (removeFieldWhenEmpty && packages.length === 0) delete settings.packages;
    writeFileAtomic(settingsPath, `${stringifyJsonc(settings, null, 2)}\n`);
    return true;
}

export function writeEidnaraConfig(
    configPath: string,
    options: {
        historianModel: string;
        historianThinkingLevel?: string;
        sidekickEnabled: boolean;
        sidekickModel?: string;
        sidekickThinkingLevel?: string;
        modelRefToCanonical?: (ref: string) => string;
    },
): void {
    const config = readJsoncConfigForUpdate(configPath);
    ensureDir(dirname(configPath));

    if (!config.$schema) {
        config.$schema =
            "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json";
    }

    // Model pickers return harness-native provider IDs. Persist only canonical
    // OpenCode-form IDs so every harness reads the same shared config.
    const toCanonical = options.modelRefToCanonical ?? piModelRefToCanonical;
    // comment-json keeps a section's comments as symbol-keyed metadata on the
    // parsed object; a spread copy would drop them from the rewritten file.
    const historian = isRecord(config.historian) ? config.historian : {};
    historian.model = toCanonical(options.historianModel);
    historian.thinking_level = options.historianThinkingLevel;
    delete historian.disable;
    delete historian.enabled;
    config.historian = compactObject(historian);

    const sidekick = isRecord(config.sidekick) ? config.sidekick : {};
    sidekick.model =
        options.sidekickEnabled && options.sidekickModel
            ? toCanonical(options.sidekickModel)
            : undefined;
    sidekick.thinking_level = options.sidekickEnabled ? options.sidekickThinkingLevel : undefined;
    sidekick.disable = options.sidekickEnabled ? undefined : true;
    sidekick.enabled = undefined;
    config.sidekick = compactObject(sidekick);
    writeFileAtomic(configPath, `${stringifyJsonc(config, null, 2)}\n`);
}

/**
 * GitHub Copilot reasoning models need an explicit thinking level: the
 * Copilot API injects "minimal" as the default and then rejects it (400).
 * Other providers resolve their own default, so the prompt is skipped.
 */
async function pickCopilotThinkingLevel(
    prompts: PromptIO,
    role: "historian" | "sidekick",
    model: string,
): Promise<string | undefined> {
    if (!model.startsWith("github-copilot/")) return undefined;
    prompts.log.warn(
        `GitHub Copilot reasoning models require an explicit thinking level.\n` +
            `Without it, Copilot injects "minimal" as a default — which it then rejects with a 400 error.`,
    );
    return prompts.selectOne(`Select thinking level for ${role}`, [
        {
            label: "medium — good quality, moderate cost (Recommended)",
            value: "medium",
            recommended: true,
        },
        { label: "low — faster, less thorough", value: "low" },
        { label: "high — best quality, slowest", value: "high" },
        { label: "off — no thinking, fastest (not recommended)", value: "off" },
    ]);
}

/**
 * The read is lenient because dry runs skip config validation; an unreadable
 * config resolves to the schema defaults (both enabled).
 */
function readEidnaraModes(configPath: string): EidnaraModes {
    const config = readJsoncLenient(configPath).value;
    const enabled = config.enabled !== false;
    return {
        enabled,
        compactionEnabled: enabled && isCompactionEnabled(config),
        memoryEnabled: enabled && (!isRecord(config.memory) || config.memory.enabled !== false),
    };
}

export async function runSetup(options: RunSetupOptions = {}): Promise<number> {
    const prompts = options.prompts ?? (await getDefaultPrompts());
    const env = options.env ?? DEFAULT_ENV;
    const host = options.host ?? DEFAULT_HOST;
    const dryRun = options.dryRun === true;

    prompts.intro(`Eidnara for ${host.displayName} — Setup`);
    if (dryRun) {
        prompts.log.warn("Dry run — no files will be written and no package will be registered.");
    }

    const spinner = prompts.spinner();
    spinner.start(`Checking ${host.displayName} installation`);
    const binary = env.detectPiBinary();
    if (!binary) {
        spinner.stop(`${host.displayName} not found`);
        prompts.log.warn(
            `Could not find \`${host.cliName}\` on PATH or in standard user install directories.`,
        );
        prompts.log.message(`Install ${host.displayName} first, then rerun setup.`);
        prompts.outro(`Setup stopped — install ${host.displayName} and try again`);
        return 1;
    }

    const version = env.getPiVersion(binary.path);
    spinner.stop(
        version
            ? `${host.displayName} ${version} detected at ${binary.path}`
            : `${host.displayName} detected at ${binary.path}`,
    );

    if (version && host.minimumVersion && comparePiVersion(version, host.minimumVersion) < 0) {
        prompts.log.warn(
            host.versionWarning?.(version, host.minimumVersion) ??
                `${host.displayName} ${version} is older than required ${host.minimumVersion}.`,
        );
        const proceed = await prompts.confirm(
            "Continue with setup anyway? (subagents may fail at runtime)",
            false,
        );
        if (!proceed) {
            // A non-zero code keeps the dispatcher from printing next steps after nothing was written.
            prompts.outro(`Setup cancelled — upgrade ${host.displayName} and try again.`);
            return 1;
        }
    }

    spinner.start(`Fetching available ${host.displayName} models`);
    const allModels = env.getAvailableModels(binary.path);
    spinner.stop(`Found ${allModels.length} model choices`);

    const settingsPath = env.paths.getPiUserExtensionsPath();
    const configPath = env.paths.getPiUserConfigPath();
    const configureHost = await prompts.confirm(
        `Configure ${host.displayName} to load Eidnara?`,
        true,
    );
    // The read-only check runs in dry-run mode too, so a dry run predicts the refusal a real run would make.
    try {
        // Validate every target this run will write before writing any of
        // them, so a later invalid target cannot leave a partial setup.
        // The host settings file is a target only when registration is on.
        assertJsoncConfigsParseable(configureHost ? [settingsPath, configPath] : [configPath]);
    } catch (error) {
        prompts.log.error(error instanceof Error ? error.message : String(error));
        prompts.outro("Setup stopped — fix the malformed config and rerun setup.");
        return 1;
    }
    if (configureHost && dryRun) {
        prompts.log.message(
            `[dry-run] would register ${host.packageSource} for ${host.displayName} in ${settingsPath}`,
        );
    } else if (!configureHost) {
        prompts.log.warn(`Skipped ${host.displayName} package registration.`);
    }

    const historianModel = await pickModel(prompts, allModels, "historian");
    const historianThinkingLevel = await pickCopilotThinkingLevel(
        prompts,
        "historian",
        historianModel,
    );

    const sidekickEnabled = await prompts.confirm("Enable sidekick for /ctx-aug?", false);
    const sidekickModel = sidekickEnabled
        ? await pickModel(prompts, allModels, "sidekick")
        : undefined;
    const sidekickThinkingLevel = sidekickModel
        ? await pickCopilotThinkingLevel(prompts, "sidekick", sidekickModel)
        : undefined;

    const eidnara = readEidnaraModes(configPath);
    if (!eidnara.enabled) {
        prompts.log.warn(
            `Eidnara is disabled (\`enabled: false\`) in ${configPath}; setup keeps that setting and leaves ${host.displayName}'s native context managers on.`,
        );
    }
    // Project config is a per-project opt-out layered over the shared config.
    // Native host settings are global, so `eidnara` follows the shared config.
    const projectConfigPath = resolveEidnaraProjectConfigPath(process.cwd());
    if (readJsoncLenient(projectConfigPath).value.enabled === false) {
        prompts.log.warn(
            `Eidnara is disabled (\`enabled: false\`) by the project config ${projectConfigPath}; it will not run in this project after setup.`,
        );
    }
    const rollbackHost =
        (await host.beforeWrite?.({
            binaryPath: binary.path,
            cwd: process.cwd(),
            prompts,
            dryRun,
            configureHost,
            eidnara,
        })) ?? (async () => {});
    if (rollbackHost === false) {
        prompts.outro(`Setup stopped — could not configure ${host.displayName}.`);
        return 1;
    }

    let registration: PluginEntryResult | undefined;
    try {
        if (dryRun) {
            prompts.log.message(`[dry-run] would write Eidnara config to ${configPath}`);
        } else {
            if (configureHost) {
                registration = await host.ensurePluginEntry(settingsPath);
                if (!registration.ok) throw new Error(registration.message);
                prompts.log.success(registration.message);
            }
            writeEidnaraConfig(configPath, {
                historianModel,
                historianThinkingLevel,
                sidekickEnabled,
                sidekickModel,
                sidekickThinkingLevel,
                modelRefToCanonical: host.modelRefToCanonical,
            });
            prompts.log.success(`Config written to ${configPath}`);
        }
    } catch (error) {
        prompts.log.error(error instanceof Error ? error.message : String(error));
        if (registration?.ok && host.rollbackPluginEntry) {
            try {
                await host.rollbackPluginEntry(registration);
            } catch (rollbackError) {
                prompts.log.error(
                    rollbackError instanceof Error ? rollbackError.message : String(rollbackError),
                );
                prompts.log.warn(
                    `Left ${host.displayName} native settings as configured for Eidnara: ` +
                        `restoring them while the Eidnara plugin is still registered would run two context managers at once.`,
                );
                prompts.outro(
                    `Setup stopped — undo the ${host.displayName} plugin registration by hand, then rerun setup.`,
                );
                return 1;
            }
        }
        try {
            await rollbackHost();
        } catch (rollbackError) {
            prompts.log.error(
                rollbackError instanceof Error ? rollbackError.message : String(rollbackError),
            );
            prompts.outro(
                `Setup stopped — ${host.displayName} changes were only partly rolled back; finish the steps above by hand.`,
            );
            return 1;
        }
        prompts.outro(`Setup stopped — rolled back ${host.displayName} changes.`);
        return 1;
    }

    const thinkingSuffix = (level: string | undefined): string =>
        level ? ` (thinking: ${level})` : "";
    const summary = [
        `${host.displayName} plugin: ${configureHost ? settingsPath : "skipped"}`,
        `Eidnara config: ${configPath}`,
        `Historian: ${historianModel}${thinkingSuffix(historianThinkingLevel)}`,
        sidekickEnabled
            ? `Sidekick: ${sidekickModel}${thinkingSuffix(sidekickThinkingLevel)}`
            : "Sidekick: disabled",
    ].join("\n");

    prompts.note(summary, dryRun ? "Configuration (dry run — not written)" : "Configuration");
    prompts.outro(
        dryRun
            ? "Dry run complete — nothing was written."
            : `Start a ${host.displayName} session and try /ctx-aug`,
    );
    return 0;
}
