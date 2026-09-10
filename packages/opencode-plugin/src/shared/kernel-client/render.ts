import { ANTI_MEMORY_CATEGORY, antiMemoryExpired, parseAntiMemoryContent } from "./anti-memory";
import { guidanceFor, type MemoryState } from "./state";
import { isMemoryDecisionRow, type ReadRow } from "./wire";

/** The rows list, search, and status counters serve: memory-domain decisions minus anti-memories past their rendered expiry. An unparseable summary never counts as expired. */
export function isServedMemoryDecisionRow(row: ReadRow, nowMs: number): boolean {
    if (!isMemoryDecisionRow(row)) return false;
    if (row.decision?.decision_kind !== ANTI_MEMORY_CATEGORY) return true;
    try {
        return !antiMemoryExpired(parseAntiMemoryContent(row.decision.payload.summary), nowMs);
    } catch {
        return true;
    }
}

/** One sentence for a tool result. */
export function renderToolStateText(state: MemoryState): string {
    return guidanceFor(state).tool;
}
