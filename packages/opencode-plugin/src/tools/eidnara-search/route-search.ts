import { Deadline } from "../../shared/host-client";
import {
    isAvailable,
    isMemoryDecisionRow,
    type KernelClient,
    type LaneStatus,
    type MemoryState,
    type ReadRow,
    ROUTE_LANES,
    type RouteCausality,
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
    resolveContests,
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
    causality: RouteCausality;
}

function refusedState(state: MemoryState): RouteSearch {
    return { kind: "refused", text: `Error: ${renderToolStateText(state)}` };
}

function servesRevision(row: ReadRow, revision: number): boolean {
    return (
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
    /** The budget a caller that runs several passes of one search gives them together; absent, the pass starts its own. */
    deadline?: Deadline;
}): Promise<RouteSearch> {
    const translated = routeQueryText(args.query);
    if (!translated.ok) return { kind: "refused", text: translated.text };
    const deadline = args.deadline ?? Deadline.start(ROUTE_SEARCH_DEADLINE_MS);
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
            known.conflicting ||=
                known.revision !== reference.source_revision ||
                known.causality !== reference.causality;
            continue;
        }
        decisions.set(reference.decision_object_id, {
            objectId: reference.decision_object_id,
            revision: reference.source_revision,
            lanes: new Set(entry.lanes),
            labeled: reference.visibility === "labeled",
            conflicting: false,
            causality: reference.causality,
        });
    }
    const ranking = [...decisions.values()];
    const notes: string[] = [];
    if (ranked.entries.length > 0 && decisions.size === 0) {
        notes.push("Memory: the fused ranking matched no current memory decisions.");
    }
    if (ranked.degraded) notes.push(degradedNote(ranked.lanes));
    if (ranked.truncated) {
        notes.push("Memory: the fused ranking was truncated at the route's result bound.");
    }
    if (ranking.length === 0) return { kind: "ranked", results: [], notes };

    // The limit counts memory decisions the read's store state holds. A ranked decision of another domain, such as the statement record that approves a memory and repeats its text, takes no place. A ranked decision whose row the read does not return takes no place either: that store state no longer serves it, its domain is unknown, and the search reports it as changed after ranking. Each read names as many of the next ranked decisions as places remain, and every read after the first reads the store state the first one answered at, so a commit between two reads changes nothing the search renders.
    const nowMs = args.nowMs ?? Date.now();
    const results: KernelMemorySearchResult[] = [];
    const stale: string[] = [];
    const unresolvedObjectIds: string[] = [];
    // A whole number of places, so each pass names at least one decision.
    let places = Math.floor(args.limit);
    let snapshot: number | null = null;
    const hydrated: ReadRow[] = [];
    for (let next = 0; places > 0 && next < ranking.length; ) {
        const selected = ranking.slice(next, next + places);
        if (selected.length === 0) break;
        next += selected.length;
        const read = await readObjectRowsChunked({
            client: args.client,
            surface: "explicit_search",
            gated: false,
            objectIds: selected.map((decision) => decision.objectId),
            signal: args.signal,
            deadline,
            asOf: snapshot,
        });
        if (!read.ok) return refusedState(read.state);
        if (
            args.client.connectionIdentity() !== connection ||
            (snapshot !== null && read.knownAsOf !== snapshot)
        ) {
            return refusedState(unavailable("snapshot_diverged"));
        }
        snapshot = read.knownAsOf;
        hydrated.push(...read.rows);
        unresolvedObjectIds.push(...read.unresolvedObjectIds);
        const rows = new Map(read.rows.map((row) => [row.object.object_id, row]));
        const unresolved = new Set(read.unresolvedObjectIds);
        for (const decision of selected) {
            if (unresolved.has(decision.objectId)) {
                places -= 1;
                continue;
            }
            const row = rows.get(decision.objectId);
            if (row === undefined) {
                stale.push(decision.objectId);
                continue;
            }
            if (!isMemoryDecisionRow(row)) continue;
            places -= 1;
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
                provenance: decision.causality,
            });
        }
    }
    const contestNote = await resolveContests({
        client: args.client,
        results,
        rows: hydrated,
        asOf: snapshot,
        ...(args.signal ? { signal: args.signal } : {}),
        deadline,
    });
    // The contest read is one more read of the search: a daemon connection that changed since the ranking refuses the search, as it does for the hydration reads.
    if (args.client.connectionIdentity() !== connection) {
        return refusedState(unavailable("snapshot_diverged"));
    }
    if (contestNote) notes.push(contestNote);
    if (stale.length > 0) {
        notes.push(
            `Memory: ${stale.length} ranked ${stale.length === 1 ? "memory" : "memories"} changed or became unreadable after ranking and ${stale.length === 1 ? "was" : "were"} not rendered.`,
        );
    }
    if (unresolvedObjectIds.length > 0) {
        notes.push(`Memory: unresolved ranked memory ids: ${unresolvedObjectIds.join(", ")}`);
    }
    return { kind: "ranked", results, notes };
}
