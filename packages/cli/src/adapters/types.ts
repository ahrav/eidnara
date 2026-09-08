export type HarnessKind = "opencode" | "pi" | "omp";

export interface HarnessConfigPaths {
    /** configDir identifies the primary configuration directory. */
    configDir: string;
    pluginConfigPath: string;
    /** Absent when the environment provides no absolute home, so no user tier exists to read or write. */
    eidnaraConfigPath: string | undefined;
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
    /**
     * On an error result: the registration command ran and its undo failed or could not be
     * verified, so the plugin may still be active. A caller must then leave the host's native
     * context managers disabled rather than run two managers at once.
     */
    pluginMayBeActive?: boolean;
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
