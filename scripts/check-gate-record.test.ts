import { describe, expect, test } from "bun:test";

import { type Change, checkGateRecord, lastRound } from "./check-gate-record";

const ORDER = ["c1", "c2", "c3"];

function change(files: string[], edits: Record<string, string[]>): Change {
    return {
        files,
        resolve: (revision) => ORDER.find((commit) => commit.startsWith(revision.slice(0, 2))),
        isAncestor: (ancestor, descendant) => ORDER.indexOf(ancestor) <= ORDER.indexOf(descendant),
        changedBetween: (from, to) =>
            ORDER.slice(ORDER.indexOf(from) + 1, ORDER.indexOf(to) + 1).flatMap((commit) => edits[commit] ?? []),
    };
}

describe("lastRound", () => {
    test("reads the highest round, with or without backticks", () => {
        const body = "Round 1 reviewed `aaaaaaa`..`bbbbbbb`: ponytail\nround 2 reviewed ccccccc..ddddddd: rust\n";
        expect(lastRound(body)).toEqual({ number: 2, base: "ccccccc", head: "ddddddd" });
    });

    test("ignores a range without the round prefix", () => {
        expect(lastRound("Reviewed aaaaaaa..bbbbbbb")).toBeUndefined();
    });
});

describe("checkGateRecord", () => {
    test("a Markdown-only change needs no record", () => {
        expect(checkGateRecord("", "c3", change(["docs/a.md"], {}))).toEqual([]);
    });

    test("a code change with no recorded round fails", () => {
        const findings = checkGateRecord("Pre-PR gate: ran", "c3", change(["src/a.rs"], {}));
        expect(findings).toHaveLength(1);
        expect(findings[0]).toContain("records no");
    });

    test("a round that names the head passes", () => {
        const body = "Round 1 reviewed c0aaaaa..c3aaaaa: rust";
        expect(checkGateRecord(body, "c3", change(["src/a.rs"], { c3: ["src/a.rs"] }))).toEqual([]);
    });

    test("a code commit after the reviewed head fails and names the file", () => {
        const body = "Round 1 reviewed c0aaaaa..c2aaaaa: rust";
        const findings = checkGateRecord(
            body,
            "c3",
            change(["src/a.rs", "docs/a.md"], { c3: ["src/a.rs", "docs/a.md"] }),
        );
        expect(findings.slice(1)).toEqual(["  src/a.rs"]);
    });

    test("a later Markdown commit and a main merge that touches only other files pass", () => {
        const body = "Round 1 reviewed c0aaaaa..c2aaaaa: rust";
        const edits = { c3: ["docs/a.md", "crates/other/src/lib.rs"] };
        expect(checkGateRecord(body, "c3", change(["src/a.rs", "docs/a.md"], edits))).toEqual([]);
    });

    test("a reviewed head outside the branch history fails", () => {
        const body = "Round 1 reviewed c0aaaaa..ffaaaaa: rust";
        const findings = checkGateRecord(body, "c3", change(["src/a.rs"], {}));
        expect(findings[0]).toContain("not a commit");
    });

    test("a reviewed head that is not an ancestor of the head fails", () => {
        const body = "Round 1 reviewed c0aaaaa..c3aaaaa: rust";
        const findings = checkGateRecord(body, "c2", change(["src/a.rs"], {}));
        expect(findings[0]).toContain("not an ancestor");
    });
});
