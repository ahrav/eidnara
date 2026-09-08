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
