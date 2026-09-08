/**
 * The harness-neutral body of `ctx_search`. The OpenCode and Pi tool wrappers
 * parse their own argument shapes and call `executeCtxSearch`, so both
 * harnesses rank, pack, and word the states of daemon-served memories the
 * same way. commentlint: allow(JUDGE)
 */

import { resolveProjectRootDirectory } from "../../features/context/project-root";
import {
    isAvailable,
    type MemoryState,
    type ReadRow,
    renderToolStateText,
} from "../../shared/kernel-client";
import {
    boundDynamicField,
    describeQueryBoundsViolation,
    normalizeSearchResultLimit,
} from "./bounds";
import {
    type KernelMemorySearchResult,
    parseObjectIdQuery,
    readObjectRowsChunked,
    searchKernelMemoryRows,
} from "./kernel-memory-search";
import { normalizeCtxSearchArgs, prepareQueryFromNormalizedArgs } from "./query-input";
import { type ExplicitDeliveryReason, packSearchResults } from "./render";
import type { CtxSearchArgs, CtxSearchSource, CtxSearchToolDeps } from "./types";

const VALID_SOURCES: ReadonlySet<CtxSearchSource> = new Set(["memory"]);

/**
 * `undefined` means `sources` was omitted; preserve [] so the search reads no sources.
 * */
function normalizeSources(sources?: string[]): CtxSearchSource[] | undefined {
    if (sources === undefined) return undefined;
    const result: CtxSearchSource[] = [];
    const seen = new Set<CtxSearchSource>();
    for (const source of sources) {
        if (VALID_SOURCES.has(source as CtxSearchSource)) {
            const typed = source as CtxSearchSource;
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

/** The wrappers fall back to raw arguments when schema parsing fails, so `sources` is validated as an array of supported names before any iteration: a non-array would throw an unhandled TypeError, and a misspelled name silently dropped would search nothing and report a misleading "no results". The raw array is caller-controlled, so the error names at most `MAX_ECHOED_UNKNOWN_SOURCES` field-bounded values and counts the rest rather than repeating arbitrarily long input. Answers the error text or `null` when valid. commentlint: allow(JUDGE) */
function invalidSourcesError(sources: unknown): string | null {
    if (sources === undefined) return null;
    if (!Array.isArray(sources)) {
        return "Error: 'sources' must be an array of source names.";
    }
    const unknown = sources.filter(
        (source) => typeof source !== "string" || !VALID_SOURCES.has(source as CtxSearchSource),
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

export interface CtxSearchCallContext {
    sessionID: string;
    directory: string;
    /** The host tool call's abort signal, forwarded into the kernel read. */
    abort?: AbortSignal;
}

/**
 * `complete` includes rendered text, the pre-pack ranking, and results whose blocks survived packing.
 * Search failures throw instead of returning an empty ranking.
 */
export type CtxSearchExecution =
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

export async function executeCtxSearch(
    deps: CtxSearchToolDeps,
    rawArgs: CtxSearchArgs,
    toolContext: CtxSearchCallContext,
): Promise<CtxSearchExecution> {
    const args = normalizeCtxSearchArgs(rawArgs);
    // Non-string model-supplied `query` values are treated as missing rather than throwing.
    const preflight = prepareQueryFromNormalizedArgs(args);
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

    const projectPath = deps.resolveProjectPath(toolContext.directory);
    if (!projectPath) {
        return { status: "invalid", text: "Error: Could not resolve project identity for search." };
    }
    const projectRoot = resolveProjectRootDirectory(toolContext.directory);

    const completeFrom = (
        results: KernelMemorySearchResult[],
        memoryNote?: string,
    ): CtxSearchExecution => {
        const packed = packSearchResults(query, results, memoryNote);
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
    // An id query filters the read so a named object beyond the daemon's row cap still resolves; the chunked read splits a list over the client's filter bound into filtered requests and splits on byte-budget truncation, so every named id resolves or is reported unresolved by name. commentlint: allow(JUDGE)
    const idQuery = parseObjectIdQuery(query);
    let memoryRows: ReadRow[] = [];
    let memoryState: MemoryState | null = null;
    let memoryTruncated = false;
    let unresolvedObjectIds: string[] = [];
    if (idQuery) {
        const read = await readObjectRowsChunked({
            client,
            surface: "explicit_search",
            gated: true,
            objectIds: idQuery,
            ...(toolContext.abort ? { signal: toolContext.abort } : {}),
        });
        if (read.ok) {
            memoryRows = read.rows;
            unresolvedObjectIds = read.unresolvedObjectIds;
        } else {
            memoryState = read.state;
        }
    } else {
        const read = await client.read({
            surface: "explicit_search",
            gated: true,
            ...(toolContext.abort ? { signal: toolContext.abort } : {}),
        });
        if (isAvailable(read)) {
            memoryRows = read.rows;
            memoryTruncated = read.truncated;
        } else {
            memoryState = read.state;
        }
    }
    if (memoryState) {
        return { status: "invalid", text: `Error: ${renderToolStateText(memoryState)}` };
    }
    const notes: string[] = [];
    if (unresolvedObjectIds.length > 0) {
        notes.push(
            `Memory: unresolved object id${unresolvedObjectIds.length === 1 ? "" : "s"} (the daemon read stayed truncated): ${unresolvedObjectIds.join(", ")}`,
        );
    }
    // A truncated snapshot drops the oldest rows, so a lexical hit that lives only in an omitted memory is silently missing; the note keeps "no results" from reading as a complete search. commentlint: allow(JUDGE)
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
