import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import { isRecord } from "@eidnara/opencode/shared/record-type-guard";
import { readJsoncLenient } from "./jsonc-config";

export interface EidnaraModes {
    enabled: boolean;
    compactionEnabled: boolean;
    memoryEnabled: boolean;
}

/**
 * Reads the shared user config only: setup edits global host settings, so a project-tier opt-out must not switch a native manager back on for every other project. commentlint: allow(JUDGE)
 * A missing or unreadable config resolves to the schema defaults (everything enabled).
 */
export function readEidnaraModes(configPath: string): EidnaraModes {
    const config = readJsoncLenient(configPath).value;
    const enabled = config.enabled !== false;
    return {
        enabled,
        compactionEnabled: enabled && isCompactionEnabled(config),
        memoryEnabled: enabled && (!isRecord(config.memory) || config.memory.enabled !== false),
    };
}

/**
 * Host native settings are global, so setup decides from the shared config
 * and only reports a project-tier disagreement.
 */
export function projectModeOverrides(projectConfigPath: string, shared: EidnaraModes): string[] {
    const project = readJsoncLenient(projectConfigPath).value;
    const overrides: string[] = [];
    if (typeof project.enabled === "boolean" && project.enabled !== shared.enabled) {
        overrides.push(`enabled: ${project.enabled}`);
    }
    const memory = isRecord(project.memory) ? project.memory.enabled : undefined;
    if (typeof memory === "boolean" && memory !== shared.memoryEnabled) {
        overrides.push(`memory.enabled: ${memory}`);
    }
    return overrides;
}

/** With `enabled: false` the plugin skips every hook, so a native manager must stay on regardless of `compaction.enabled`. */
export function compactionEnabledFor(config: { enabled?: unknown; compaction?: unknown }): boolean {
    if (config.enabled === false) return false;
    return isCompactionEnabled({
        compaction: isRecord(config.compaction) ? config.compaction : null,
    });
}
