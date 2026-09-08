export type HarnessKind = "opencode" | "pi" | "omp";

export interface HarnessConfigPaths {
    /** configDir identifies the primary configuration directory. */
    configDir: string;
    pluginConfigPath: string;
    eidnaraConfigPath: string;
    /**
     * secondaryConfigPath is null when the harness has no equivalent.
     */
    secondaryConfigPath: string | null;
}

export interface PluginEntryResult {
    ok: boolean;
    action: "added" | "updated" | "already_present" | "error";
    /** message provides a human-readable result summary. */
    message: string;
    configPath: string;
}

export interface HarnessAdapter {
    readonly kind: HarnessKind;
    readonly displayName: string;
    readonly pluginPackageName: string;

    isInstalled(): boolean;

    hasPluginEntry(): boolean;

    /** getConfigPaths remains callable when the harness is not installed. */
    getConfigPaths(): HarnessConfigPaths;

    /** ensurePluginEntry is idempotent. */
    ensurePluginEntry(): Promise<PluginEntryResult>;

    getLogPath(): string;
}
