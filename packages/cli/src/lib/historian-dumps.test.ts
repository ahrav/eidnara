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

describe("parseHistorianDumpMeta", () => {
    it("reports a dump with no usable compartment as a parse error", () => {
        const dir = dumpDir();
        const garbage = join(dir, "garbage.xml");
        const truncated = join(dir, "truncated.xml");
        writeFileSync(garbage, "not a historian response");
        writeFileSync(truncated, '<output><compartment start="1" end="3" title="t"><p1>cut off');

        expect(parseHistorianDumpMeta(garbage)).toEqual({
            error: "no usable <compartment> elements",
        });
        expect(parseHistorianDumpMeta(truncated)).toEqual({
            error: "no usable <compartment> elements",
        });
    });

    it("reports a read failure as a parse error", () => {
        const dir = dumpDir();
        const meta = parseHistorianDumpMeta(join(dir, "missing.xml"));
        expect("error" in meta).toBe(true);
    });

    it("counts gaps and overlaps against the running coverage end", () => {
        const dir = dumpDir();
        const path = join(dir, "ranges.xml");
        writeFileSync(path, [compartment(1, 10), compartment(2, 3), compartment(8, 12)].join("\n"));

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
        const path = join(dir, "gap.xml");
        writeFileSync(
            path,
            [compartment(1, 10), compartment(2, 3), compartment(12, 14)].join("\n"),
        );

        const meta = parseHistorianDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.ordinalGapCount).toBe(1);
        expect(meta.ordinalOverlapCount).toBe(1);
    });

    it("counts neither for contiguous ranges", () => {
        const dir = dumpDir();
        const path = join(dir, "contiguous.xml");
        writeFileSync(path, [compartment(1, 4), compartment(5, 9), compartment(10, 10)].join("\n"));

        const meta = parseHistorianDumpMeta(path);
        if ("error" in meta) throw new Error(meta.error);
        expect(meta.ordinalGapCount).toBe(0);
        expect(meta.ordinalOverlapCount).toBe(0);
    });
});

describe("listDumpsInDir", () => {
    it("keeps healthy dumps when one entry cannot be statted", () => {
        const dir = dumpDir();
        writeFileSync(join(dir, "good.xml"), compartment(1, 2));
        symlinkSync(join(dir, "gone.xml"), join(dir, "dangling.xml"));

        const listing = listDumpsInDir(dir, 5);
        expect(listing.count).toBe(1);
        expect(listing.recent.map((entry) => entry.name)).toEqual(["good.xml"]);
        expect(listing.recent[0].meta?.compartmentCount).toBe(1);
    });

    it("sorts newest first, caps at the limit, and carries parse errors per dump", () => {
        const dir = dumpDir();
        const older = join(dir, "older.xml");
        const newer = join(dir, "newer.xml");
        const broken = join(dir, "broken.xml");
        writeFileSync(older, compartment(1, 2));
        writeFileSync(newer, compartment(3, 4));
        writeFileSync(broken, "no compartments here");
        const base = Date.now() / 1000;
        utimesSync(older, base - 300, base - 300);
        utimesSync(newer, base - 60, base - 60);
        utimesSync(broken, base - 120, base - 120);

        const listing = listDumpsInDir(dir, 2);
        expect(listing.count).toBe(3);
        expect(listing.recent.map((entry) => entry.name)).toEqual(["newer.xml", "broken.xml"]);
        expect(listing.recent[0].meta?.compartmentCount).toBe(1);
        expect(listing.recent[1].parseError).toBe("no usable <compartment> elements");
    });

    it("returns an empty listing for a missing directory", () => {
        const dir = dumpDir();
        expect(listDumpsInDir(join(dir, "absent"), 5)).toEqual({ count: 0, recent: [] });
    });
});
