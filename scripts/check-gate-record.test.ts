import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type Change, checkGateRecord, gitChange, lastRound } from "./check-gate-record";

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

    test("keeps the highest round when rounds appear out of order, and accepts a ref-name base", () => {
        const body = "Round 2 reviewed origin/main..ddddddd: rust\nRound 1 reviewed aaaaaaa..bbbbbbb: ponytail\n";
        expect(lastRound(body)).toEqual({ number: 2, base: "origin/main", head: "ddddddd" });
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

describe("gitChange", () => {
    let repo: string;

    function git(...args: string[]): string {
        const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
        if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
        return result.stdout.trim();
    }

    function commit(message: string, files: Record<string, string>, remove: string[] = []): string {
        for (const [name, content] of Object.entries(files)) {
            writeFileSync(join(repo, name), content);
            git("add", name);
        }
        for (const name of remove) git("rm", "-q", name);
        git("commit", "-q", "-m", message);
        return git("rev-parse", "HEAD");
    }

    beforeEach(() => {
        repo = mkdtempSync(join(tmpdir(), "check-gate-record-"));
        git("init", "-q", "-b", "main");
        git("config", "user.email", "test@example.com");
        git("config", "user.name", "test");
    });

    afterEach(() => {
        rmSync(repo, { recursive: true, force: true });
    });

    test("a deletion after the reviewed head fails when rename detection would pair it with a copy", () => {
        const lines = `${Array.from({ length: 200 }, (_, index) => `${index}`).join("\n")}\n`;
        const base = commit("base", { "x.rs": lines });
        const reviewed = commit("copy", { "y.rs": `${lines}extra\n` });
        const head = commit("delete", {}, ["x.rs"]);
        const findings = checkGateRecord(`Round 1 reviewed ${base}..${reviewed}: rust`, head, gitChange(base, head, repo));
        expect(findings.slice(1)).toEqual(["  x.rs"]);
    });

    test("a merge of main that touches only other files and a later Markdown edit keep the round current", () => {
        const base = commit("base", { "a.rs": "a\n", "b.rs": "b\n" });
        git("checkout", "-q", "-b", "pr");
        const reviewed = commit("pr change", { "a.rs": "a2\n" });
        git("checkout", "-q", "main");
        commit("main change", { "b.rs": "b2\n" });
        git("checkout", "-q", "pr");
        git("merge", "-q", "--no-edit", "main");
        const head = commit("docs", { "notes.md": "notes\n" });
        const body = `Round 1 reviewed ${base}..${reviewed}: rust`;
        expect(checkGateRecord(body, head, gitChange("main", head, repo))).toEqual([]);
    });

    test("a merge of main that touches a file the pull request changes needs a new round", () => {
        const base = commit("base", { "a.rs": "a\n1\n2\n3\n" });
        git("checkout", "-q", "-b", "pr");
        const reviewed = commit("pr change", { "a.rs": "a2\n1\n2\n3\n" });
        git("checkout", "-q", "main");
        commit("main change", { "a.rs": "a\n1\n2\n3b\n" });
        git("checkout", "-q", "pr");
        git("merge", "-q", "--no-edit", "main");
        const head = git("rev-parse", "HEAD");
        const body = `Round 1 reviewed ${base}..${reviewed}: rust`;
        expect(checkGateRecord(body, head, gitChange("main", head, repo)).slice(1)).toEqual(["  a.rs"]);
    });
});
