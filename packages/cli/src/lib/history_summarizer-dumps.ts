/**
 * HistorySummarizer dump inspection shared by the OpenCode and Pi diagnostics: one
 * metadata shape, one parser, and one directory walker so the two doctors
 * cannot drift on what a dump summary contains.
 */

import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import { parseHistorySegmentOutput } from "@eidnara/opencode/hooks/context/history_segment-parser";
import { readRegularFileSync } from "@eidnara/opencode/shared/regular-file";

export interface HistorySummarizerDumpSummary {
    name: string;
    ageMinutes: number;
    sizeKb: number;
    /** Parsed metadata — only structural fields, never raw XML content. */
    meta?: HistorySummarizerDumpMeta;
    /** Why the dump is unusable: unreadable, not one `<output>` document, or no history_segment. */
    parseError?: string;
}

export interface HistorySummarizerDumpMeta {
    /** Number of <history_segment> elements found. */
    history_segmentCount: number;
    /** Smallest start ordinal across history_segments, or null if none. */
    minStart: number | null;
    /** Largest end ordinal across history_segments, or null if none. */
    maxEnd: number | null;
    /** Value of <unprocessed_from> tag, if present. */
    unprocessedFrom: number | null;
    /** Number of <fact> items grouped by category. */
    factCountByCategory: Record<string, number>;
    /** Number of <user_observations> items. */
    userObservationCount: number;
    /** Total number of history_segment ordinal gaps (missing ranges between consecutive history_segments). */
    ordinalGapCount: number;
    /** Total number of overlapping history_segment ranges. */
    ordinalOverlapCount: number;
}

export function fileSize(path: string): number {
    try {
        return statSync(path).size;
    } catch {
        return 0;
    }
}

// Mirrors `output_document_regex` and `output_tag_regex` in the daemon's `history_summarizer_validate`.
const OUTPUT_DOCUMENT_REGEX = /^\s*<output(?:\s[^>]*)?>([\s\S]*)<\/output\s*>\s*$/i;
const OUTPUT_TAG_REGEX = /<\/?output(?:\s[^>]*)?>/i;

export function parseHistorySummarizerDumpMeta(
    path: string,
): HistorySummarizerDumpMeta | { error: string } {
    try {
        const xml = readRegularFileSync(path);
        const root = OUTPUT_DOCUMENT_REGEX.exec(xml);
        if (!root) {
            return { error: "not one complete <output> document" };
        }
        if (OUTPUT_TAG_REGEX.test(root[1])) {
            return { error: "more than one <output> document" };
        }
        const parsed = parseHistorySegmentOutput(xml);
        if (parsed.history_segments.length === 0) {
            return { error: "no usable <history_segment> elements" };
        }
        const factCountByCategory: Record<string, number> = {};
        for (const fact of parsed.facts) {
            factCountByCategory[fact.category] = (factCountByCategory[fact.category] ?? 0) + 1;
        }
        const starts = parsed.history_segments.map((c) => c.startMessage);
        const ends = parsed.history_segments.map((c) => c.endMessage);
        let gaps = 0;
        let overlaps = 0;
        // HistorySegments are sorted by startMessage. Compare each range with coveredEnd, not the
        // previous range's end: [1,10], [2,3], [8,12] has no gap.
        let coveredEnd = parsed.history_segments[0].endMessage;
        for (let i = 1; i < parsed.history_segments.length; i++) {
            const curr = parsed.history_segments[i];
            if (curr.startMessage > coveredEnd + 1) gaps += 1;
            else if (curr.startMessage <= coveredEnd) overlaps += 1;
            coveredEnd = Math.max(coveredEnd, curr.endMessage);
        }
        return {
            history_segmentCount: parsed.history_segments.length,
            minStart: starts.length > 0 ? Math.min(...starts) : null,
            maxEnd: ends.length > 0 ? Math.max(...ends) : null,
            unprocessedFrom: parsed.unprocessedFrom,
            factCountByCategory,
            userObservationCount: parsed.userObservations.length,
            ordinalGapCount: gaps,
            ordinalOverlapCount: overlaps,
        };
    } catch (error) {
        return { error: error instanceof Error ? error.message : String(error) };
    }
}

/**
 * Walk a directory's `*.xml` files and return them as HistorySummarizerDumpSummary
 * entries, sorted newest-first. Returns up to `limit` entries. An entry that
 * cannot be statted, such as a dangling symlink or a dump removed mid-walk, is
 * left out; the rest of the listing survives.
 *
 * Both dump walkers call this so the dump-listing shape lives in one place.
 */
export function listDumpsInDir(
    dir: string,
    limit: number,
): { count: number; recent: HistorySummarizerDumpSummary[] } {
    try {
        const entries = readdirSync(dir)
            .filter((name) => name.endsWith(".xml"))
            .flatMap((name) => {
                // An entry removed or made unreadable mid-walk drops only itself, not the directory.
                try {
                    const stat = statSync(join(dir, name));
                    // Reading a FIFO with no writer blocks, so only regular files are dumps.
                    if (!stat.isFile()) return [];
                    return [{ name, mtime: stat.mtimeMs, sizeKb: Math.round(stat.size / 1024) }];
                } catch {
                    return [];
                }
            })
            .sort((a, b) => b.mtime - a.mtime);

        const now = Date.now();
        const recent: HistorySummarizerDumpSummary[] = entries.slice(0, limit).map((entry) => {
            const meta = parseHistorySummarizerDumpMeta(join(dir, entry.name));
            const summary: HistorySummarizerDumpSummary = {
                name: entry.name,
                ageMinutes: Math.round((now - entry.mtime) / 60000),
                sizeKb: entry.sizeKb,
            };
            if ("error" in meta) {
                summary.parseError = meta.error;
            } else {
                summary.meta = meta;
            }
            return summary;
        });
        return { count: entries.length, recent };
    } catch {
        return { count: 0, recent: [] };
    }
}

export interface HistorySummarizerDumpBucketLike {
    directory: string;
    count: number;
    recent: HistorySummarizerDumpSummary[];
}

export interface HistorySummarizerDumpsReportLike {
    byProject: HistorySummarizerDumpBucketLike[];
    legacyDumps: { dir: string; count: number };
}

export function scopeDumpBucketsToSession<
    T extends { primarySessionId: string; sessionIds: string[] },
>(buckets: readonly T[], sessionId: string): T[] {
    return buckets
        .filter((bucket) => bucket.sessionIds.includes(sessionId))
        .map((bucket) => ({ ...bucket, primarySessionId: sessionId, sessionIds: [sessionId] }));
}

/** Check lines a doctor prints for the dumps it found: a warning for the total, info lines for the three newest per project. */
export function describeHistorySummarizerDumps(
    report: HistorySummarizerDumpsReportLike,
): Array<{ status: "warn" | "info"; message: string }> {
    const lines: Array<{ status: "warn" | "info"; message: string }> = [];
    if (report.byProject.length > 0) {
        const totalCount = report.byProject.reduce((sum, bucket) => sum + bucket.count, 0);
        lines.push({
            status: "warn",
            message: `HistorySummarizer debug dumps: ${totalCount} file(s) across ${report.byProject.length} project(s)`,
        });
        for (const bucket of report.byProject) {
            lines.push({
                status: "info",
                message: `  [${bucket.directory}] ${bucket.count} file(s)`,
            });
            for (const dump of bucket.recent.slice(0, 3)) {
                const age = dump.ageMinutes;
                const ageStr = age < 60 ? `${age}m ago` : `${Math.round(age / 60)}h ago`;
                lines.push({ status: "info", message: `    ${dump.name} (${ageStr})` });
            }
            if (bucket.count > 3) {
                lines.push({ status: "info", message: `    ... and ${bucket.count - 3} more` });
            }
        }
    }
    if (report.legacyDumps.count > 0) {
        lines.push({
            status: "info",
            message: `Legacy history_summarizer dumps (pre-v0.18.x): ${report.legacyDumps.count} file(s) in ${report.legacyDumps.dir}`,
        });
    }
    return lines;
}
