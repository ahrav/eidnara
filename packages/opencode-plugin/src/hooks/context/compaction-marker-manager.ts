import {
    closeCompactionMarkerDb,
    removeEidnaraOwnedCompactionMarkers,
} from "../../features/context/compaction-marker";
import { getHarness } from "../../shared/harness";
import { sessionLog } from "../../shared/logger";

export const MARKER_SUMMARY_TEXT =
    "[Compacted by eidnara — session history is managed by the plugin]";

/** Failures are logged, not thrown: session deletion and native compaction must not fail on a marker row. commentlint: allow(JUDGE) */
export function removeCompactionMarkerForSession(sessionId: string): void {
    // Only OpenCode sessions have marker rows in `opencode.db`.
    if (getHarness() !== "opencode") return;
    try {
        const result = removeEidnaraOwnedCompactionMarkers(sessionId, MARKER_SUMMARY_TEXT);
        if (result.removedRows > 0) {
            sessionLog(
                sessionId,
                `compaction-marker: removed ${result.removedLineages} lineage(s) (${result.removedRows} rows) on session cleanup`,
            );
        }
    } catch (error) {
        sessionLog(sessionId, "compaction-marker: removal failed during session cleanup:", error);
    }
}

export function closeCompactionMarkerConnection(): void {
    closeCompactionMarkerDb();
}
