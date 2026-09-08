import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { listDumpsInDir, parseHistorianDumpMeta } from "./historian-dumps";

const roots: string[] = [];

afterEach(() => {
    for (const root of roots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

function dumpDir(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-historian-dumps-"));
    roots.push(root);
    return root;
}

function compartment(start: number, end: number): string {
    return `<compartment start="${start}" end="${end}" title="c${start}">body ${start}</compartment>`;
}

function outputDocument(...ranges: [number, number][]): string {
    const body = ranges.map(([start, end]) => compartment(start, end)).join("\n");
    return `<output>\n<compartments>\n${body}\n</compartments>\n</output>\n`;
}

function writeDump(dir: string, name: string, text: string): string {
    const path = join(dir, name);
    writeFileSync(path, text);
    return path;
}

describe("parseHistorianDumpMeta", () => {
    it("rejects text that is not one complete <output> document", () => {
        const dir = dumpDir();
        const garbage = writeDump(dir, "garbage.xml", "not a historian response");
        const noRoot = writeDump(dir, "no-root.xml", compartment(1, 2));
        const cutInCompartment = writeDump(
            dir,
            "cut-in-compartment.xml",
            '<output><compartments><compartment start="1" end="3" title="t"><p1>cut off',
        );
        const cutAfterCompartment = writeDump(
            dir,
            "cut-after-compartment.xml",
            `<output>\n<compartments>\n${compartment(1, 4)}\n</compartments>\n<facts>`,
        );
        const trailingProse = writeDump(
            dir,
            "trailing-prose.xml",
            `${outputDocument([1, 2])}Here is the summary you asked for.`,
        );

        const expected = { error: "not one complete <output> document" };
        expect(parseHistorianDumpMeta(garbage)).toEqual(expected);
        expect(parseHistorianDumpMeta(noRoot)).toEqual(expected);
        expect(parseHistorianDumpMeta(cutInCompartment)).toEqual(expected);
        expect(parseHistorianDumpMeta(cutAfterCompartment)).toEqual(expected);
        expect(parseHistorianDumpMeta(trailingProse)).toEqual(expected);
    });

    it("rejects a second <output> document inside the root", () => {
        const dir = dumpDir();
        const doubled = writeDump(dir, "doubled.xml", `<output>${outputDocument([1, 2])}</output>`);
        expect(parseHistorianDumpMeta(doubled)).toEqual({
            error: "more than one <output> document",
        });
    });

    it("rejects a complete document with no usable compartment", () => {
        const dir = dumpDir();
        const empty = writeDump(dir, "empty.xml", "<output><compartments></compartments></output>");
        const unparseable = writeDump(
            dir,
            "unparseable.xml",
            '<output><compartment end="3" title="missing start">x</compartment></output>',
        );
        const expected = { error: "no usable <compartment> elements" };
        expect(parseHistorianDumpMeta(empty)).toEqual(expected);
        expect(parseHistorianDumpMeta(unparseable)).toEqual(expected);
    });

    it("accepts surrounding whitespace and attributes on the root", () => {
        const dir = dumpDir();
        const path = writeDump(
            dir,
            "attrs.xml",
            `\n  <output version="2">${compartment(1, 2)}</output >\n\n`,
        );
        const meta = parseHistorianDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.compartmentCount).toBe(1);
    });

    it("reports a read failure as a parse error", () => {
        const dir = dumpDir();
        const meta = parseHistorianDumpMeta(join(dir, "missing.xml"));
        expect("error" in meta).toBe(true);
    });

    it("counts gaps and overlaps against the running coverage end", () => {
        const dir = dumpDir();
        const path = writeDump(dir, "ranges.xml", outputDocument([1, 10], [2, 3], [8, 12]));

        const meta = parseHistorianDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.compartmentCount).toBe(3);
        expect(meta.minStart).toBe(1);
        expect(meta.maxEnd).toBe(12);
        expect(meta.ordinalGapCount).toBe(0);
        expect(meta.ordinalOverlapCount).toBe(2);
    });

    it("counts a real gap after an enclosing range", () => {
        const dir = dumpDir();
        const path = writeDump(dir, "gap.xml", outputDocument([1, 10], [2, 3], [12, 14]));

        const meta = parseHistorianDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.ordinalGapCount).toBe(1);
        expect(meta.ordinalOverlapCount).toBe(1);
    });

    it("counts neither for contiguous ranges", () => {
        const dir = dumpDir();
        const path = writeDump(dir, "contiguous.xml", outputDocument([1, 4], [5, 9], [10, 10]));

        const meta = parseHistorianDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.ordinalGapCount).toBe(0);
        expect(meta.ordinalOverlapCount).toBe(0);
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
        expect(listing.recent[0].meta?.compartmentCount).toBe(1);
    });

    it("sorts newest first, caps at the limit, and carries parse errors per dump", () => {
        const dir = dumpDir();
        const older = writeDump(dir, "older.xml", outputDocument([1, 2]));
        const newer = writeDump(dir, "newer.xml", outputDocument([3, 4]));
        const broken = writeDump(dir, "broken.xml", "no compartments here");
        const base = Date.now() / 1000;
        utimesSync(older, base - 300, base - 300);
        utimesSync(newer, base - 60, base - 60);
        utimesSync(broken, base - 120, base - 120);

        const listing = listDumpsInDir(dir, 2);
        expect(listing.count).toBe(3);
        expect(listing.recent.map((entry) => entry.name)).toEqual(["newer.xml", "broken.xml"]);
        expect(listing.recent[0].meta?.compartmentCount).toBe(1);
        expect(listing.recent[1].parseError).toBe("not one complete <output> document");
    });

    it("returns an empty listing for a missing directory", () => {
        const dir = dumpDir();
        expect(listDumpsInDir(join(dir, "absent"), 5)).toEqual({ count: 0, recent: [] });
    });
});
