import { formatMemoryCount, type SidebarSnapshot } from "../shared/rpc-types";

export interface CompactionOffSidebarRow {
    label: "Memories" | "Notes" | "Archived compartments";
    value: string;
}

/** Prefers `native_context_usage_percentage`, measured against the unreserved model window; `contextLimit` subtracts the output reservation. commentlint: allow(JUDGE) */
export function nativeCompactionContextLabel(snapshot: SidebarSnapshot): string {
    const native = snapshot.native_context_usage_percentage;
    if (typeof native === "number" && Number.isFinite(native)) {
        return `Context: ${native.toFixed(1)}% · native compaction`;
    }
    if (snapshot.contextLimit <= 0) return "Context: unknown · native compaction";
    const percentage = (snapshot.inputTokens / snapshot.contextLimit) * 100;
    return `Context: ${percentage.toFixed(1)}% · native compaction`;
}

/** Recovers the unreserved window from `inputTokens / percentage`, falling back to the reserved `contextLimit`. commentlint: allow(JUDGE) */
export function nativeContextLimit(snapshot: SidebarSnapshot): number {
    const native = snapshot.native_context_usage_percentage;
    if (
        typeof native === "number" &&
        Number.isFinite(native) &&
        native > 0 &&
        snapshot.inputTokens > 0
    ) {
        return Math.round(snapshot.inputTokens / (native / 100));
    }
    return snapshot.contextLimit;
}

export function compactionOffSidebarRows(snapshot: SidebarSnapshot): CompactionOffSidebarRow[] {
    const rows: CompactionOffSidebarRow[] = [
        { label: "Memories", value: formatMemoryCount(snapshot) },
    ];
    // A zero count can mean the producer had no source for it, not an empty collection, so the row stays hidden rather than rendering a misleading 0.
    if (snapshot.sessionNoteCount > 0) {
        rows.push({ label: "Notes", value: String(snapshot.sessionNoteCount) });
    }
    const archivedCount = snapshot.archivedCompartmentCount ?? 0;
    if (archivedCount > 0) {
        rows.push({ label: "Archived compartments", value: String(archivedCount) });
    }
    return rows;
}
