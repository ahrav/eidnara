import { type ConfigAdmission, loadUserTierConfigDetailed } from "@eidnara/opencode/config";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import { isRecord } from "@eidnara/opencode/shared/record-type-guard";
import { readJsoncLenient } from "./jsonc-config";

export interface EidnaraModes {
    enabled: boolean;
    compactionEnabled: boolean;
    memoryEnabled: boolean;
    admission: ConfigAdmission;
}

/**
 * Reads the shared user config only: setup edits global host settings, so a project-tier opt-out must not switch a native manager back on for every other project.
 * `planned.summarizerModel` supplies the `history_summarizer.model` setup writes, so the compaction mode reflects the post-setup config.
 */
export function readEidnaraModes(
    configPath: string | undefined,
    planned: { summarizerModel?: string } = {},
): EidnaraModes {
    const { config, admission } = loadUserTierConfigDetailed(configPath);
    const enabled = config.enabled !== false;
    const history_summarizer =
        planned.summarizerModel === undefined
            ? config.history_summarizer
            : { ...config.history_summarizer, model: planned.summarizerModel };
    return {
        enabled,
        compactionEnabled: compactionEnabledFor({ ...config, history_summarizer }),
        memoryEnabled: enabled && config.memory.enabled !== false,
        admission,
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
    // A project-tier compaction mode is not compared: `project-security.ts` strips it before the config is resolved.
    const memory = isRecord(project.memory) ? project.memory.enabled : undefined;
    if (typeof memory === "boolean" && memory !== shared.memoryEnabled) {
        overrides.push(`memory.enabled: ${memory}`);
    }
    return overrides;
}

/** With `enabled: false` the plugin skips every hook, so a native manager must stay on whatever the compaction setting says. */
export function compactionEnabledFor(config: {
    enabled?: unknown;
    compaction?: unknown;
    history_summarizer?: unknown;
}): boolean {
    if (config.enabled === false) return false;
    return isCompactionEnabled({
        compaction: isRecord(config.compaction) ? config.compaction : null,
        history_summarizer: config.history_summarizer,
    });
}
