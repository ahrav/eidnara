/**
 * The harness-neutral body of `eidnara_search`. The OpenCode and Pi tool wrappers
 * parse their own argument shapes and call `executeEidnaraSearch`, so both
 * harnesses rank, pack, and word the states of daemon-served memories the
 * same way.
 */

import { resolveProjectRootDirectory } from "../../features/context/project-identity";
import { getErrorMessage } from "../../shared/error-message";
import {
    isAvailable,
    type MemoryState,
    type ReadRow,
    renderToolStateText,
} from "../../shared/kernel-client";
import {
    boundDynamicField,
    describeQueryBoundsViolation,
    type ExplicitQueryPreparation,
    normalizeSearchResultLimit,
} from "./bounds";
import {
    type KernelMemorySearchResult,
    parseObjectIdQuery,
    readObjectRowsChunked,
    searchKernelMemoryRows,
} from "./kernel-memory-search";
import { normalizeEidnaraSearchArgs, prepareQueryFromNormalizedArgs } from "./query-input";
import { type ExplicitDeliveryReason, type PackedSearchResults, packSearchResults } from "./render";
import type { EidnaraSearchArgs, EidnaraSearchSource, EidnaraSearchToolDeps } from "./types";

const VALID_SOURCES: ReadonlySet<EidnaraSearchSource> = new Set(["memory"]);

/** Refusing keeps `MAX_QUERY_TOKENS` and `MAX_RENDERED_RESULT_TOKENS` enforced when the counter cannot run. */
function tokenCountingUnavailable(error: unknown): EidnaraSearchExecution {
    return {
        status: "invalid",
        text: `Error: eidnara_search cannot bound its query or output because native token counting is unavailable; no results were returned. ${getErrorMessage(error)}`,
    };
}

/**
 * `undefined` means `sources` was omitted; preserve [] so the search reads no sources.
 * */
function normalizeSources(sources?: string[]): EidnaraSearchSource[] | undefined {
    if (sources === undefined) return undefined;
    const result: EidnaraSearchSource[] = [];
    const seen = new Set<EidnaraSearchSource>();
    for (const source of sources) {
        if (VALID_SOURCES.has(source as EidnaraSearchSource)) {
            const typed = source as EidnaraSearchSource;
            if (!seen.has(typed)) {
                seen.add(typed);
                result.push(typed);
            }
        }
    }
    return result;
}

/** Unknown source names echoed back in the validation error; the rest are counted. */
const MAX_ECHOED_UNKNOWN_SOURCES = 3;

/** The wrappers fall back to raw arguments when schema parsing fails, so `sources` is validated as an array of supported names before any iteration: a non-array would throw an unhandled TypeError, and a misspelled name silently dropped would search nothing and report a misleading "no results". The raw array is caller-controlled, so the error names at most `MAX_ECHOED_UNKNOWN_SOURCES` field-bounded values and counts the rest rather than repeating arbitrarily long input. Answers the error text or `null` when valid. */
function invalidSourcesError(sources: unknown): string | null {
    if (sources === undefined) return null;
    if (!Array.isArray(sources)) {
        return "Error: 'sources' must be an array of source names.";
    }
    const unknown = sources.filter(
        (source) => typeof source !== "string" || !VALID_SOURCES.has(source as EidnaraSearchSource),
    );
    if (unknown.length > 0) {
        const echoed = unknown
            .slice(0, MAX_ECHOED_UNKNOWN_SOURCES)
            .map((source) =>
                typeof source === "string"
                    ? JSON.stringify(boundDynamicField(source))
                    : boundDynamicField(JSON.stringify(source) ?? String(source)),
            );
        const elided = unknown.length - echoed.length;
        const more = elided > 0 ? `, and ${elided} more` : "";
        return `Error: unknown source${unknown.length === 1 ? "" : "s"}: ${echoed.join(", ")}${more}. Supported sources: ${[...VALID_SOURCES].join(", ")}.`;
    }
    return null;
}

export interface EidnaraSearchCallContext {
    sessionID: string;
    directory: string;
    /** The host tool call's abort signal, forwarded into the kernel read. */
    abort?: AbortSignal;
}

/**
 * `complete` includes rendered text, the pre-pack ranking, and results whose blocks survived packing.
 * Search failures throw instead of returning an empty ranking.
 */
export type EidnaraSearchExecution =
    | { status: "invalid"; text: string }
    | {
          status: "complete";
          text: string;
          prePack: KernelMemorySearchResult[];
          delivered: KernelMemorySearchResult[];
          tokenCount: number;
          omittedCount: number;
          reason: ExplicitDeliveryReason;
      };

/** One memory read for the search: the rows it returned, or the state that refused it. */
type MemoryRead =
    | { rows: ReadRow[]; truncated: boolean; unresolvedObjectIds: string[]; state: null }
    | { state: MemoryState };

export async function executeEidnaraSearch(
    deps: EidnaraSearchToolDeps,
    rawArgs: EidnaraSearchArgs,
    toolContext: EidnaraSearchCallContext,
): Promise<EidnaraSearchExecution> {
    const args = normalizeEidnaraSearchArgs(rawArgs);
    // Non-string model-supplied `query` values are treated as missing rather than throwing.
    let preflight: ExplicitQueryPreparation;
    try {
        preflight = prepareQueryFromNormalizedArgs(args);
    } catch (error) {
        return tokenCountingUnavailable(error);
    }
    if (!preflight.ok) {
        return { status: "invalid", text: `Error: ${describeQueryBoundsViolation(preflight)}` };
    }
    const query = preflight.query;
    if (!query) {
        return { status: "invalid", text: "Error: 'query' is required." };
    }
    const sourcesError = invalidSourcesError((rawArgs as { sources?: unknown }).sources);
    if (sourcesError) {
        return { status: "invalid", text: sourcesError };
    }

    const directory = deps.resolveSessionDirectory
        ? await deps.resolveSessionDirectory(toolContext.sessionID, toolContext.directory)
        : toolContext.directory;
    const projectPath = deps.resolveProjectPath(directory);
    if (!projectPath) {
        return { status: "invalid", text: "Error: Could not resolve project identity for search." };
    }
    const projectRoot = resolveProjectRootDirectory(directory);

    const completeFrom = (
        results: KernelMemorySearchResult[],
        memoryNote?: string,
    ): EidnaraSearchExecution => {
        let packed: PackedSearchResults;
        try {
            packed = packSearchResults(query, results, memoryNote);
        } catch (error) {
            return tokenCountingUnavailable(error);
        }
        return {
            status: "complete",
            text: packed.text,
            prePack: results,
            delivered: packed.delivered,
            tokenCount: packed.tokenCount,
            omittedCount: packed.omittedCount,
            reason: packed.reason,
        };
    };

    // An explicit `sources: []` names no source, so the search completes empty without a daemon round trip.
    const requestedSources = normalizeSources(args.sources);
    if (requestedSources !== undefined && !requestedSources.includes("memory")) {
        return completeFrom([]);
    }

    // The `memory` source is served by the daemon through the kernel client, gated on
    // freshness: a lagging projector answers `stale` and the search says so instead of
    // ranking rows that may miss recent writes.
    const client = deps.kernelClient({
        sessionId: toolContext.sessionID,
        projectRoot,
    });
    // An id query filters the read so a named object beyond the daemon's row cap still resolves; the chunked read splits a list over the client's filter bound into filtered requests and splits on byte-budget truncation, so every named id resolves or is reported unresolved by name.
    const idQuery = parseObjectIdQuery(query);
    // The gate judges the lag of registered consumers. A deployment with no consumer at all
    // (no admitted search projection) has no derived state that can lag behind the canonical
    // rows this read returns, so the same read is repeated ungated and the result says that
    // freshness was not judged, instead of the search failing for the deployment's lifetime.
    const readMemory = async (gated: boolean): Promise<MemoryRead> => {
        const signal = toolContext.abort ? { signal: toolContext.abort } : {};
        if (idQuery) {
            const read = await readObjectRowsChunked({
                client,
                surface: "explicit_search",
                gated,
                objectIds: idQuery,
                ...signal,
            });
            return read.ok
                ? {
                      rows: read.rows,
                      truncated: false,
                      unresolvedObjectIds: read.unresolvedObjectIds,
                      state: null,
                  }
                : { state: read.state };
        }
        const read = await client.read({ surface: "explicit_search", gated, ...signal });
        return isAvailable(read)
            ? { rows: read.rows, truncated: read.truncated, unresolvedObjectIds: [], state: null }
            : { state: read.state };
    };
    let memory = await readMemory(true);
    let freshnessNote: string | undefined;
    if (memory.state?.kind === "unavailable" && memory.state.reason === "no_required_consumer") {
        freshnessNote = `Memory: ${renderToolStateText(memory.state)} Results are read from the canonical tip.`;
        memory = await readMemory(false);
    }
    if (memory.state) {
        return { status: "invalid", text: `Error: ${renderToolStateText(memory.state)}` };
    }
    const { rows: memoryRows, truncated: memoryTruncated, unresolvedObjectIds } = memory;
    const notes: string[] = [];
    if (freshnessNote) notes.push(freshnessNote);
    if (unresolvedObjectIds.length > 0) {
        notes.push(
            `Memory: unresolved object id${unresolvedObjectIds.length === 1 ? "" : "s"} (the daemon read stayed truncated): ${unresolvedObjectIds.join(", ")}`,
        );
    }
    // A truncated snapshot drops the oldest rows, so a lexical hit that lives only in an omitted memory is silently missing; the note keeps "no results" from reading as a complete search.
    if (memoryTruncated) {
        notes.push("Memory: the memory read was truncated; older memories were not searched.");
    }
    const memoryNote = notes.length > 0 ? notes.join("\n") : undefined;
    const hits = searchKernelMemoryRows({
        rows: memoryRows,
        query,
        limit: normalizeSearchResultLimit(args.limit),
        excludeObjectIds: new Set(),
    });
    return completeFrom(hits ?? [], memoryNote);
}
