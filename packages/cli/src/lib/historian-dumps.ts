/**
 * Historian dump inspection shared by the OpenCode and Pi diagnostics: one
 * metadata shape, one parser, and one directory walker so the two doctors
 * cannot drift on what a dump summary contains.
 */

import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import { parseCompartmentOutput } from "@eidnara/opencode/hooks/context/compartment-parser";
import { readRegularFileSync } from "@eidnara/opencode/shared/regular-file";

export interface HistorianDumpSummary {
    name: string;
    ageMinutes: number;
    sizeKb: number;
    /** Parsed metadata — only structural fields, never raw XML content. */
    meta?: HistorianDumpMeta;
    /** Why the dump is unusable: unreadable, not one `<output>` document, or no compartment. */
    parseError?: string;
}

export interface HistorianDumpMeta {
    /** Number of <compartment> elements found. */
    compartmentCount: number;
    /** Smallest start ordinal across compartments, or null if none. */
    minStart: number | null;
    /** Largest end ordinal across compartments, or null if none. */
    maxEnd: number | null;
    /** Value of <unprocessed_from> tag, if present. */
    unprocessedFrom: number | null;
    /** Number of <fact> items grouped by category. */
    factCountByCategory: Record<string, number>;
    /** Number of <user_observations> items. */
    userObservationCount: number;
    /** Total number of compartment ordinal gaps (missing ranges between consecutive compartments). */
    ordinalGapCount: number;
    /** Total number of overlapping compartment ranges. */
    ordinalOverlapCount: number;
}

export function fileSize(path: string): number {
    try {
        return statSync(path).size;
    } catch {
        return 0;
    }
}

// Mirrors `output_document_regex` and `output_tag_regex` in the daemon's `historian_validate`.
const OUTPUT_DOCUMENT_REGEX = /^\s*<output(?:\s[^>]*)?>([\s\S]*)<\/output\s*>\s*$/i;
const OUTPUT_TAG_REGEX = /<\/?output(?:\s[^>]*)?>/i;

export function parseHistorianDumpMeta(path: string): HistorianDumpMeta | { error: string } {
    try {
        const xml = readRegularFileSync(path);
        const root = OUTPUT_DOCUMENT_REGEX.exec(xml);
        if (!root) {
            return { error: "not one complete <output> document" };
        }
        if (OUTPUT_TAG_REGEX.test(root[1])) {
            return { error: "more than one <output> document" };
        }
        const parsed = parseCompartmentOutput(xml);
        if (parsed.compartments.length === 0) {
            return { error: "no usable <compartment> elements" };
        }
        const factCountByCategory: Record<string, number> = {};
        for (const fact of parsed.facts) {
            factCountByCategory[fact.category] = (factCountByCategory[fact.category] ?? 0) + 1;
        }
        const starts = parsed.compartments.map((c) => c.startMessage);
        const ends = parsed.compartments.map((c) => c.endMessage);
        let gaps = 0;
        let overlaps = 0;
        // Compartments are sorted by startMessage. Compare each range with coveredEnd, not the
        // previous range's end: [1,10], [2,3], [8,12] has no gap.
        let coveredEnd = parsed.compartments[0].endMessage;
        for (let i = 1; i < parsed.compartments.length; i++) {
            const curr = parsed.compartments[i];
            if (curr.startMessage > coveredEnd + 1) gaps += 1;
            else if (curr.startMessage <= coveredEnd) overlaps += 1;
            coveredEnd = Math.max(coveredEnd, curr.endMessage);
        }
        return {
            compartmentCount: parsed.compartments.length,
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
 * Walk a directory's `*.xml` files and return them as HistorianDumpSummary
 * entries, sorted newest-first. Returns up to `limit` entries. An entry that
 * cannot be statted, such as a dangling symlink or a dump removed mid-walk, is
 * left out; the rest of the listing survives.
 *
 * Both dump walkers call this so the dump-listing shape lives in one place.
 */
export function listDumpsInDir(
    dir: string,
    limit: number,
): { count: number; recent: HistorianDumpSummary[] } {
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
        const recent: HistorianDumpSummary[] = entries.slice(0, limit).map((entry) => {
            const meta = parseHistorianDumpMeta(join(dir, entry.name));
            const summary: HistorianDumpSummary = {
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

export interface HistorianDumpBucketLike {
    directory: string;
    count: number;
    recent: HistorianDumpSummary[];
}

export interface HistorianDumpsReportLike {
    byProject: HistorianDumpBucketLike[];
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
export function describeHistorianDumps(
    report: HistorianDumpsReportLike,
): Array<{ status: "warn" | "info"; message: string }> {
    const lines: Array<{ status: "warn" | "info"; message: string }> = [];
    if (report.byProject.length > 0) {
        const totalCount = report.byProject.reduce((sum, bucket) => sum + bucket.count, 0);
        lines.push({
            status: "warn",
            message: `Historian debug dumps: ${totalCount} file(s) across ${report.byProject.length} project(s)`,
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
            message: `Legacy historian dumps (pre-v0.18.x): ${report.legacyDumps.count} file(s) in ${report.legacyDumps.dir}`,
        });
    }
    return lines;
}
