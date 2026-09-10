import { formatMemoryStatus, type SidebarSnapshot } from "../shared/rpc-types";

export interface CompactionOffSidebarRow {
    label: "Memories" | "Notes" | "Archived compartments";
    value: string;
}

/** Disabling Eidnara compaction does not enable the host's; the suffix says which owner, if any, the host reported. */
function compactionOwnerSuffix(snapshot: SidebarSnapshot): string {
    return snapshot.native_compaction_active === false
        ? "no active compaction"
        : "native compaction";
}

/** Prefers `native_context_usage_percentage`, measured against the unreserved model window; `contextLimit` subtracts the output reservation. */
export function nativeCompactionContextLabel(snapshot: SidebarSnapshot): string {
    const owner = compactionOwnerSuffix(snapshot);
    const native = snapshot.native_context_usage_percentage;
    if (typeof native === "number" && Number.isFinite(native)) {
        return `Context: ${native.toFixed(1)}% · ${owner}`;
    }
    if (snapshot.contextLimit <= 0) return `Context: unknown · ${owner}`;
    const percentage = (snapshot.inputTokens / snapshot.contextLimit) * 100;
    return `Context: ${percentage.toFixed(1)}% · ${owner}`;
}

/** Recovers the unreserved window from `inputTokens / percentage`, falling back to the reserved `contextLimit`. */
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
        { label: "Memories", value: formatMemoryStatus(snapshot) },
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
