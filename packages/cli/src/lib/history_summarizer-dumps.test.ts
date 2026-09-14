import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { listDumpsInDir, parseHistorySummarizerDumpMeta } from "./history_summarizer-dumps";

const roots: string[] = [];

afterEach(() => {
    for (const root of roots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

function dumpDir(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-history_summarizer-dumps-"));
    roots.push(root);
    return root;
}

function history_segment(start: number, end: number): string {
    return `<history_segment start="${start}" end="${end}" title="c${start}">body ${start}</history_segment>`;
}

function outputDocument(...ranges: [number, number][]): string {
    const body = ranges.map(([start, end]) => history_segment(start, end)).join("\n");
    return `<output>\n<history_segments>\n${body}\n</history_segments>\n</output>\n`;
}

function writeDump(dir: string, name: string, text: string): string {
    const path = join(dir, name);
    writeFileSync(path, text);
    return path;
}

describe("parseHistorySummarizerDumpMeta", () => {
    it("rejects text that is not one complete <output> document", () => {
        const dir = dumpDir();
        const garbage = writeDump(dir, "garbage.xml", "not a history_summarizer response");
        const noRoot = writeDump(dir, "no-root.xml", history_segment(1, 2));
        const cutInHistorySegment = writeDump(
            dir,
            "cut-in-history_segment.xml",
            '<output><history_segments><history_segment start="1" end="3" title="t"><p1>cut off',
        );
        const cutAfterHistorySegment = writeDump(
            dir,
            "cut-after-history_segment.xml",
            `<output>\n<history_segments>\n${history_segment(1, 4)}\n</history_segments>\n<facts>`,
        );
        const trailingProse = writeDump(
            dir,
            "trailing-prose.xml",
            `${outputDocument([1, 2])}Here is the summary you asked for.`,
        );

        const expected = { error: "not one complete <output> document" };
        expect(parseHistorySummarizerDumpMeta(garbage)).toEqual(expected);
        expect(parseHistorySummarizerDumpMeta(noRoot)).toEqual(expected);
        expect(parseHistorySummarizerDumpMeta(cutInHistorySegment)).toEqual(expected);
        expect(parseHistorySummarizerDumpMeta(cutAfterHistorySegment)).toEqual(expected);
        expect(parseHistorySummarizerDumpMeta(trailingProse)).toEqual(expected);
    });

    it("rejects a second <output> document inside the root", () => {
        const dir = dumpDir();
        const doubled = writeDump(dir, "doubled.xml", `<output>${outputDocument([1, 2])}</output>`);
        expect(parseHistorySummarizerDumpMeta(doubled)).toEqual({
            error: "more than one <output> document",
        });
    });

    it("rejects a complete document with no usable history_segment", () => {
        const dir = dumpDir();
        const empty = writeDump(
            dir,
            "empty.xml",
            "<output><history_segments></history_segments></output>",
        );
        const unparseable = writeDump(
            dir,
            "unparseable.xml",
            '<output><history_segment end="3" title="missing start">x</history_segment></output>',
        );
        const expected = { error: "no usable <history_segment> elements" };
        expect(parseHistorySummarizerDumpMeta(empty)).toEqual(expected);
        expect(parseHistorySummarizerDumpMeta(unparseable)).toEqual(expected);
    });

    it("accepts surrounding whitespace and attributes on the root", () => {
        const dir = dumpDir();
        const path = writeDump(
            dir,
            "attrs.xml",
            `\n  <output version="2">${history_segment(1, 2)}</output >\n\n`,
        );
        const meta = parseHistorySummarizerDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.history_segmentCount).toBe(1);
    });

    it("reports a read failure as a parse error", () => {
        const dir = dumpDir();
        const meta = parseHistorySummarizerDumpMeta(join(dir, "missing.xml"));
        expect("error" in meta).toBe(true);
    });

    it("counts gaps and overlaps against the running coverage end", () => {
        const dir = dumpDir();
        const cases: {
            name: string;
            ranges: [number, number][];
            maxEnd: number;
            gaps: number;
            overlaps: number;
        }[] = [
            // The running end of [1, 10] already covers [2, 3] and part of [8, 12].
            {
                name: "overlaps.xml",
                ranges: [
                    [1, 10],
                    [2, 3],
                    [8, 12],
                ],
                maxEnd: 12,
                gaps: 0,
                overlaps: 2,
            },
            // A range beyond the enclosing range's end opens a real gap.
            {
                name: "gap.xml",
                ranges: [
                    [1, 10],
                    [2, 3],
                    [12, 14],
                ],
                maxEnd: 14,
                gaps: 1,
                overlaps: 1,
            },
            {
                name: "contiguous.xml",
                ranges: [
                    [1, 4],
                    [5, 9],
                    [10, 10],
                ],
                maxEnd: 10,
                gaps: 0,
                overlaps: 0,
            },
        ];
        for (const { name, ranges, maxEnd, gaps, overlaps } of cases) {
            const meta = parseHistorySummarizerDumpMeta(
                writeDump(dir, name, outputDocument(...ranges)),
            );
            if ("error" in meta) throw new Error(meta.error);
            expect(meta.history_segmentCount, name).toBe(3);
            expect(meta.minStart, name).toBe(1);
            expect(meta.maxEnd, name).toBe(maxEnd);
            expect(meta.ordinalGapCount, name).toBe(gaps);
            expect(meta.ordinalOverlapCount, name).toBe(overlaps);
        }
    });
});

describe("listDumpsInDir", () => {
    it("keeps healthy dumps when one entry cannot be statted", () => {
        const dir = dumpDir();
        writeDump(dir, "good.xml", outputDocument([1, 2]));
        symlinkSync(join(dir, "gone.xml"), join(dir, "dangling.xml"));

        const listing = listDumpsInDir(dir, 5);
        expect(listing.count).toBe(1);
        expect(listing.recent.map((entry) => entry.name)).toEqual(["good.xml"]);
        expect(listing.recent[0].meta?.history_segmentCount).toBe(1);
    });

    it("sorts newest first, caps at the limit, and carries parse errors per dump", () => {
        const dir = dumpDir();
        const older = writeDump(dir, "older.xml", outputDocument([1, 2]));
        const newer = writeDump(dir, "newer.xml", outputDocument([3, 4]));
        const broken = writeDump(dir, "broken.xml", "no history_segments here");
        const base = Date.now() / 1000;
        utimesSync(older, base - 300, base - 300);
        utimesSync(newer, base - 60, base - 60);
        utimesSync(broken, base - 120, base - 120);

        const listing = listDumpsInDir(dir, 2);
        expect(listing.count).toBe(3);
        expect(listing.recent.map((entry) => entry.name)).toEqual(["newer.xml", "broken.xml"]);
        expect(listing.recent[0].meta?.history_segmentCount).toBe(1);
        expect(listing.recent[1].parseError).toBe("not one complete <output> document");
    });

    it("returns an empty listing for a missing directory", () => {
        const dir = dumpDir();
        expect(listDumpsInDir(join(dir, "absent"), 5)).toEqual({ count: 0, recent: [] });
    });
});
