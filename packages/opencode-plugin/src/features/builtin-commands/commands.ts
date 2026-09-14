import { MEMORY_MARK_COMMAND, MEMORY_MARK_DESCRIPTION } from "../../shared/memory-mark-command";
import type { BuiltinCommandConfig } from "./types";

const COMPACTION_ENABLED_PATH = `compaction${".enabled"}`;

export function getEidnaraBuiltinCommands(compactionEnabled = true): BuiltinCommandConfig {
    const unavailableInCompactionOff = (command: string) =>
        `Unavailable when ${COMPACTION_ENABLED_PATH} is false: /${command} manages compacted history.`;

    return {
        "eidnara-status": {
            template: "eidnara-status",
            description: "Show Eidnara status, pending queue, cache TTL, and debug info",
        },
        "eidnara-recomp": {
            template: "eidnara-recomp",
            description: compactionEnabled
                ? "Rebuild history_segments and facts from raw history"
                : unavailableInCompactionOff("eidnara-recomp"),
        },
        "eidnara-wrapup": {
            template: "eidnara-wrapup",
            description: compactionEnabled
                ? "Compact older live history while keeping the newest messages raw"
                : unavailableInCompactionOff("eidnara-wrapup"),
        },
        "eidnara-flush": {
            template: "eidnara-flush",
            description: compactionEnabled
                ? "Force-process all pending Eidnara operations immediately"
                : unavailableInCompactionOff("eidnara-flush"),
        },
        "eidnara-aug": {
            template: "eidnara-aug",
            description:
                "Augment your prompt with project memory context via context_researcher agent",
        },
        [MEMORY_MARK_COMMAND]: {
            template: MEMORY_MARK_COMMAND,
            description: MEMORY_MARK_DESCRIPTION,
        },
    };
}
