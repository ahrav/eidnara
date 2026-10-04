import { Deadline } from "../../shared/host-client";
import {
    isAvailable,
    isMemoryDecisionRow,
    type KernelClient,
    type LaneStatus,
    type MemoryState,
    type ReadRow,
    ROUTE_LANES,
    type RouteLane,
    renderToolStateText,
    unavailable,
} from "../../shared/kernel-client";
import { antiMemoryExpired } from "../../shared/kernel-client/anti-memory";
import {
    antiMemoryPayloadOfRow,
    type KernelMemorySearchResult,
    memoryResultFromRow,
    parseObjectIdQuery,
    readObjectRowsChunked,
} from "./kernel-memory-search";

export const ROUTE_MAX_SELECTORS = 16;
export const ROUTE_SEARCH_DEADLINE_MS = 10_000;

export type RouteSearch =
    | { kind: "ranked"; results: KernelMemorySearchResult[]; notes: string[] }
    | { kind: "fallback"; reason: string }
    | { kind: "refused"; text: string };

export function routeQueryText(query: string): { ok: boolean; text: string } {
    const ids = parseObjectIdQuery(query);
    if (ids === null) return { ok: true, text: query };
    if (ids.length > ROUTE_MAX_SELECTORS) {
        return {
            ok: false,
            text: `Error: query names ${ids.length} memory ids; the search resolves at most ${ROUTE_MAX_SELECTORS} at once.`,
        };
    }
    return { ok: true, text: ids.map((id) => `id:${id}`).join(" ") };
}

interface RankedDecision {
    objectId: string;
    revision: number;
    lanes: Set<RouteLane>;
    labeled: boolean;
    conflicting: boolean;
}

function refusedState(state: MemoryState): RouteSearch {
    return { kind: "refused", text: `Error: ${renderToolStateText(state)}` };
}

function servesRevision(row: ReadRow | undefined, revision: number): row is ReadRow {
    return (
        row !== undefined &&
        isMemoryDecisionRow(row) &&
        row.object.source_revision === revision &&
        row.object.invalidated_commit_seq === null
    );
}

function degradedNote(lanes: Record<string, LaneStatus>): string {
    const affected = Object.entries(lanes)
        .filter(([, lane]) => lane.status !== "complete" && lane.status !== "undeclared")
        .map(([name, lane]) => {
            const bounds = lane.reason === null ? lane.also : [lane.reason, ...lane.also];
            return `${name} ${lane.status}${bounds.length > 0 ? ` (${bounds.join(", ")})` : ""}`;
        });
    return `Memory: the fused ranking is degraded: ${affected.join(", ")}.`;
}

export async function searchThroughRoute(args: {
    client: KernelClient;
    query: string;
    limit: number;
    signal?: AbortSignal;
    nowMs?: number;
}): Promise<RouteSearch> {
    const translated = routeQueryText(args.query);
    if (!translated.ok) return { kind: "refused", text: translated.text };
    const deadline = Deadline.start(ROUTE_SEARCH_DEADLINE_MS);
    const connection = args.client.connectionIdentity();
    const ranked = await args.client.query({
        query: translated.text,
        destination: "local",
        deadlineMs: deadline.remainingMs(),
        signal: args.signal,
    });
    if (!isAvailable(ranked)) return refusedState(ranked.state);
    if (ranked.kind === "terminal") {
        if (ranked.terminal === "disabled" || ranked.terminal === "lane_unavailable") {
            return { kind: "fallback", reason: ranked.reason ?? ranked.terminal };
        }
        return {
            kind: "refused",
            text: `Error: the memory search route answered ${ranked.terminal}; no results were returned.`,
        };
    }
    const decisions = new Map<string, RankedDecision>();
    for (const entry of ranked.entries) {
        const reference = entry.canonical;
        if (reference === null) continue;
        const known = decisions.get(reference.decision_object_id);
        if (known) {
            for (const lane of entry.lanes) known.lanes.add(lane);
            known.labeled ||= reference.visibility === "labeled";
            known.conflicting ||= known.revision !== reference.source_revision;
            continue;
        }
        decisions.set(reference.decision_object_id, {
            objectId: reference.decision_object_id,
            revision: reference.source_revision,
            lanes: new Set(entry.lanes),
            labeled: reference.visibility === "labeled",
            conflicting: false,
        });
    }
    const selected = [...decisions.values()].slice(0, args.limit);
    const notes: string[] = [];
    if (ranked.entries.length > 0 && decisions.size === 0) {
        notes.push("Memory: the fused ranking matched no current memory decisions.");
    }
    if (ranked.degraded) notes.push(degradedNote(ranked.lanes));
    if (ranked.truncated) {
        notes.push("Memory: the fused ranking was truncated at the route's result bound.");
    }
    if (selected.length === 0) return { kind: "ranked", results: [], notes };

    const read = await readObjectRowsChunked({
        client: args.client,
        surface: "explicit_search",
        gated: false,
        objectIds: selected.map((decision) => decision.objectId),
        signal: args.signal,
        deadline,
    });
    if (!read.ok) return refusedState(read.state);
    if (args.client.connectionIdentity() !== connection) {
        return refusedState(unavailable("snapshot_diverged"));
    }
    const rows = new Map(read.rows.map((row) => [row.object.object_id, row]));
    const nowMs = args.nowMs ?? Date.now();
    const results: KernelMemorySearchResult[] = [];
    const stale: string[] = [];
    const unresolved = new Set(read.unresolvedObjectIds);
    for (const decision of selected) {
        if (unresolved.has(decision.objectId)) continue;
        const row = rows.get(decision.objectId);
        if (decision.conflicting || !servesRevision(row, decision.revision)) {
            stale.push(decision.objectId);
            continue;
        }
        const antiMemory = antiMemoryPayloadOfRow(row);
        if (antiMemory && antiMemoryExpired(antiMemory, nowMs)) continue;
        const result = memoryResultFromRow(row, 1 / (results.length + 1), "fused", antiMemory);
        results.push({
            ...result,
            lanes: ROUTE_LANES.filter((lane) => decision.lanes.has(lane)),
            ...(decision.labeled ? { policyLabel: "labeled" } : {}),
        });
    }
    if (stale.length > 0) {
        notes.push(
            `Memory: ${stale.length} ranked ${stale.length === 1 ? "memory" : "memories"} changed or became unreadable after ranking and ${stale.length === 1 ? "was" : "were"} not rendered.`,
        );
    }
    if (read.unresolvedObjectIds.length > 0) {
        notes.push(`Memory: unresolved ranked memory ids: ${read.unresolvedObjectIds.join(", ")}`);
    }
    return { kind: "ranked", results, notes };
}
