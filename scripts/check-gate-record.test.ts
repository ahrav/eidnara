import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type Change, checkGateRecord, gitChange, lastRound, needsGate } from "./check-gate-record";

// Signing, hooks, and merge settings in a developer's git config can fail these commits.
const ISOLATED_GIT = { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" };

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

    test("ignores a round without the skills suffix", () => {
        expect(lastRound("Round 1 reviewed aaaaaaa..bbbbbbb\nRound 2 reviewed aaaaaaa..ccccccc:\n")).toBeUndefined();
        expect(lastRound("Round 1 reviewed `aaaaaaa`..`bbbbbbb`: no rerun - docs only")).toEqual({
            number: 1,
            base: "aaaaaaa",
            head: "bbbbbbb",
        });
    });
});

describe("needsGate", () => {
    test("gates code and the Markdown claims under docs/, and exempts guidance and other Markdown", () => {
        expect(needsGate("src/a.rs")).toBe(true);
        expect(needsGate("docs/host-wire-protocol.md")).toBe(true);
        expect(needsGate("docs/properties/tokenizer/evidence/a.md")).toBe(true);
        expect(needsGate("docs/agents/issue-tracker.md")).toBe(false);
        expect(needsGate("README.md")).toBe(false);
        expect(needsGate("crates/storage/AGENTS.md")).toBe(false);
    });
});

describe("checkGateRecord", () => {
    test("a Markdown-only change outside docs/ needs no record", () => {
        expect(checkGateRecord("", "c3", change(["README.md", "docs/agents/issue-tracker.md"], {}))).toEqual([]);
    });

    test("a change to a Markdown claim under docs/ with no recorded round fails", () => {
        const findings = checkGateRecord("", "c3", change(["docs/properties/tokenizer/README.md"], {}));
        expect(findings).toHaveLength(1);
        expect(findings[0]).toContain("records no");
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
        expect(findings[0]).toContain("not the head c3");
        expect(findings.slice(2)).toEqual(["  docs/a.md", "  src/a.rs"]);
    });

    test("a later commit that changes no gated file still needs a round for the head", () => {
        const body = "Round 1 reviewed c0aaaaa..c2aaaaa: rust";
        const edits = { c3: ["README.md", "crates/other/src/lib.rs"] };
        const findings = checkGateRecord(body, "c3", change(["src/a.rs", "README.md"], edits));
        expect(findings).toHaveLength(2);
        expect(findings[0]).toContain("not the head c3");
        expect(findings[1]).toContain("no rerun");
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
        const result = spawnSync("git", args, { cwd: repo, encoding: "utf8", env: ISOLATED_GIT });
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
        expect(findings.slice(2)).toEqual(["  x.rs"]);
    });

    test("a merge of main that touches only other files and a later Markdown edit need a round for the head", () => {
        const base = commit("base", { "a.rs": "a\n", "b.rs": "b\n" });
        git("checkout", "-q", "-b", "pr");
        const reviewed = commit("pr change", { "a.rs": "a2\n" });
        git("checkout", "-q", "main");
        commit("main change", { "b.rs": "b2\n" });
        git("checkout", "-q", "pr");
        git("merge", "-q", "--no-edit", "main");
        const head = commit("docs", { "notes.md": "notes\n" });
        const body = `Round 1 reviewed ${base}..${reviewed}: rust`;
        const findings = checkGateRecord(body, head, gitChange("main", head, repo));
        expect(findings).toHaveLength(2);
        expect(findings[0]).toContain(`not the head ${head}`);
        expect(findings[1]).toContain("no rerun");
    });

    test("a round that names the head passes", () => {
        const base = commit("base", { "a.rs": "a\n" });
        const head = commit("pr change", { "a.rs": "a2\n" });
        const body = `Round 1 reviewed ${base}..${head.slice(0, 9)}: rust`;
        expect(checkGateRecord(body, head, gitChange(base, head, repo))).toEqual([]);
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
        expect(checkGateRecord(body, head, gitChange("main", head, repo)).slice(2)).toEqual(["  a.rs"]);
    });
});
