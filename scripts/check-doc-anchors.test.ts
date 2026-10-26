import { describe, expect, test } from "bun:test";

import {
    checkAnchors,
    findAnchors,
    type Hunk,
    mapRange,
    parseHunks,
    type Repository,
} from "./check-doc-anchors";

function repository(
    files: Record<string, string>,
    changed: Record<string, string>,
    hunks: Record<string, Hunk[]>,
): Repository {
    return {
        tracked: new Set(Object.keys(files)),
        text: (path) => files[path] ?? "",
        changed: new Map(Object.entries(changed)),
        hunks: (_base, head) => hunks[head] ?? [],
    };
}

const SOURCE = `${Array.from({ length: 40 }, (_, index) => `line ${index + 1}`).join("\n")}\n`;

describe("findAnchors", () => {
    test("reads link ranges and bare ranges without counting a link twice", () => {
        const anchors = findAnchors(
            "See [`a.rs:3`](../src/a.rs#L3-L5) and `crates/b/src/b.rs:7` and c.ts:9-11.\n",
        );
        expect(anchors.map(({ form, path, start, end }) => ({ form, path, start, end }))).toEqual([
            { form: "link", path: "../src/a.rs", start: 3, end: 5 },
            { form: "bare", path: "a.rs", start: 3, end: 3 },
            { form: "bare", path: "crates/b/src/b.rs", start: 7, end: 7 },
            { form: "bare", path: "c.ts", start: 9, end: 11 },
        ]);
    });

    test("ignores versions, URLs without line fragments, and prose colons", () => {
        expect(findAnchors("Pi 0.80.2, see https://x.test/a.rs and ratio 3:4.\n")).toEqual([]);
    });

    test("refuses a partial match of a range or a decimal", () => {
        expect(findAnchors("a.rs:9-11x b.rs:3.5\n")).toEqual([]);
    });
});

describe("mapRange", () => {
    const hunks = parseHunks("@@ -2,0 +3,2 @@\n@@ -10,2 +12,1 @@\n");

    test("shifts a range below an insertion and a shrinking edit", () => {
        expect(mapRange(hunks, 20, 22)).toEqual({ kind: "moved", start: 21, end: 23 });
    });

    test("keeps a range above every hunk in place", () => {
        expect(mapRange(hunks, 1, 2)).toEqual({ kind: "same" });
    });

    test("reports a range that overlaps an edited line", () => {
        expect(mapRange(hunks, 8, 10)).toEqual({ kind: "changed" });
    });

    test("an insertion directly after the range leaves it in place", () => {
        expect(mapRange(parseHunks("@@ -5,0 +6,3 @@\n"), 4, 5)).toEqual({ kind: "same" });
    });
});

describe("checkAnchors", () => {
    test("fails an unchanged anchor in an edited document when its cited lines move", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "intro\nCheck: `src/a.rs:20-21` holds.\nnew line\n",
                "src/a.rs": SOURCE,
            },
            { "docs/p/catalog.md": "docs/p/catalog.md", "src/a.rs": "src/a.rs" },
            {
                "docs/p/catalog.md": parseHunks("@@ -2,0 +3 @@\n"),
                "src/a.rs": parseHunks("@@ -1,0 +2,2 @@\n"),
            },
        );
        expect(checkAnchors(repo)).toEqual({
            findings: ["docs/p/catalog.md:2: src/a.rs:20-21 cites lines that moved to 22-23"],
            notices: [],
        });
    });

    test("reports the same drift in an untouched document as a notice", () => {
        const repo = repository(
            { "docs/p/catalog.md": "Check: `src/a.rs:20`.\n", "src/a.rs": SOURCE },
            { "src/a.rs": "src/a.rs" },
            { "src/a.rs": parseHunks("@@ -19,2 +19,2 @@\n") },
        );
        expect(checkAnchors(repo)).toEqual({
            findings: [],
            notices: ["docs/p/catalog.md:1: src/a.rs:20 cites lines this change edits"],
        });
    });

    test("fails an added anchor past the end of its file and an added link to a missing file", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "[x](../../src/a.rs#L39-L41)\n[y](../../src/gone.rs#L1)\n",
                "src/a.rs": SOURCE,
            },
            { "docs/p/catalog.md": "" },
            {},
        );
        expect(checkAnchors(repo).findings).toEqual([
            "docs/p/catalog.md:1: ](../../src/a.rs#L39-L41) cites past the end of src/a.rs (40 lines)",
            "docs/p/catalog.md:2: ](../../src/gone.rs#L1) cites no tracked file",
        ]);
    });

    test("accepts an added anchor inside its file and leaves ambiguous bare names alone", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "intro\n`src/a.rs:40` and `lib.rs:9999`\n",
                "src/a.rs": SOURCE,
                "x/lib.rs": SOURCE,
                "y/lib.rs": SOURCE,
            },
            { "docs/p/catalog.md": "docs/p/catalog.md" },
            { "docs/p/catalog.md": parseHunks("@@ -2,0 +2 @@\n") },
        );
        expect(checkAnchors(repo)).toEqual({ findings: [], notices: [] });
    });
});
