import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import {
    checkAnchors,
    type FileChange,
    findAnchors,
    gitRepository,
    type Hunk,
    mapRange,
    parseHunks,
    type Repository,
} from "./check-doc-anchors";

// Signing, hooks, and merge settings in a developer's git config can fail these commits.
const ISOLATED_GIT = { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" };

function modified(base: string): FileChange {
    return { kind: "modified", base };
}

function repository(
    files: Record<string, string>,
    changes: Record<string, FileChange>,
    hunks: Record<string, Hunk[]>,
    removed: Record<string, string | null> = {},
    baseFiles: Record<string, string> = {},
): Repository {
    return {
        tracked: new Set(Object.keys(files)),
        text: (path) => files[path] ?? "",
        baseText: (path) => baseFiles[path] ?? "",
        changes: new Map(Object.entries(changes)),
        removed: new Map(Object.entries(removed)),
        hunks: (_base, head) => hunks[head] ?? [],
    };
}

function messages(entries: { doc: string; line: number; message: string }[]): string[] {
    return entries.map(({ doc, line, message }) => `${doc}:${line}: ${message}`);
}

const SOURCE = `${Array.from({ length: 40 }, (_, index) => `line ${index + 1}`).join("\n")}\n`;

describe("findAnchors", () => {
    test("reads link ranges and bare ranges without counting a link twice", () => {
        const anchors = findAnchors(
            "See [`a.rs:3`](../src/a.rs#L3-L5) and `crates/b/src/b.rs:7` and c.ts:9-11 and docs/x.md:4.\n",
        );
        expect(anchors.map(({ form, path, start, end }) => ({ form, path, start, end }))).toEqual([
            { form: "relative", path: "../src/a.rs", start: 3, end: 5 },
            { form: "bare", path: "a.rs", start: 3, end: 3 },
            { form: "bare", path: "crates/b/src/b.rs", start: 7, end: 7 },
            { form: "bare", path: "c.ts", start: 9, end: 11 },
            { form: "bare", path: "docs/x.md", start: 4, end: 4 },
        ]);
    });

    test("ignores versions, URLs without line fragments, and prose colons", () => {
        expect(findAnchors("Pi 0.80.2, see https://x.test/a.rs and ratio 3:4.\n")).toEqual([]);
    });

    test("ignores a commit permalink with a line fragment", () => {
        expect(findAnchors("[p](https://github.com/o/r/blob/abc/src/a.rs#L3-L4)\n")).toEqual([]);
    });

    test("refuses a partial match of a range or a decimal", () => {
        expect(findAnchors("a.rs:9-11x b.rs:3.5\n")).toEqual([]);
    });

    test("reads every range of a comma-separated anchor", () => {
        expect(findAnchors("`crates/m/src/lib.rs:131-133,255-257,300` then 4,5\n")).toEqual([
            { line: 1, text: "crates/m/src/lib.rs:131-133", path: "crates/m/src/lib.rs", form: "bare", start: 131, end: 133 },
            { line: 1, text: "crates/m/src/lib.rs:255-257", path: "crates/m/src/lib.rs", form: "bare", start: 255, end: 257 },
            { line: 1, text: "crates/m/src/lib.rs:300", path: "crates/m/src/lib.rs", form: "bare", start: 300, end: 300 },
        ]);
    });

    test("a pathless backticked range continues the preceding path in its paragraph", () => {
        const text =
            "(`crates/d/src/t.rs:2019-2027`, `:2045-2064`, `:2110`) and (`:5`)\nwrapped `:7` and 3:4 and {\"k\":9}\ntwo lines down `:8`\n\n`:11` after a blank line\n| `:12` in a table row |\n";
        expect(findAnchors(text).map(({ line, text, start, end }) => ({ line, text, start, end }))).toEqual([
            { line: 1, text: "crates/d/src/t.rs:2019-2027", start: 2019, end: 2027 },
            { line: 1, text: "crates/d/src/t.rs:2045-2064", start: 2045, end: 2064 },
            { line: 1, text: "crates/d/src/t.rs:2110", start: 2110, end: 2110 },
            { line: 1, text: "crates/d/src/t.rs:5", start: 5, end: 5 },
            { line: 2, text: "crates/d/src/t.rs:7", start: 7, end: 7 },
        ]);
    });

    test("reads lock files and token assets as bare anchors", () => {
        expect(findAnchors("`Cargo.lock:1668-1669`, `bun.lock:793`, `assets/claude.tiktoken:57534`\n").map((a) => a.text)).toEqual([
            "Cargo.lock:1668-1669",
            "bun.lock:793",
            "assets/claude.tiktoken:57534",
        ]);
    });

    test("a bare path that starts with ./ or ../ is relative to its document", () => {
        expect(findAnchors("`../../rendering/existing-checks.md:32-33` and `./local.rs:4`\n").map(({ form, path }) => ({ form, path }))).toEqual([
            { form: "relative", path: "../../rendering/existing-checks.md" },
            { form: "relative", path: "./local.rs" },
        ]);
    });

    test("reads a reference-style link definition and skips one with a scheme", () => {
        const text = "[pass]: ../../../../crates/daemon/src/lib.rs#L8191-L8271\n  [perma]: https://github.com/o/r/blob/abc/src/a.rs#L3\n[plain]: ../a.md\n";
        expect(findAnchors(text)).toEqual([
            {
                line: 1,
                text: "[pass]: ../../../../crates/daemon/src/lib.rs#L8191-L8271",
                path: "../../../../crates/daemon/src/lib.rs",
                form: "relative",
                start: 8191,
                end: 8271,
            },
        ]);
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

    test("an insertion inside the range changes it", () => {
        expect(mapRange(parseHunks("@@ -5,0 +6,3 @@\n"), 4, 8)).toEqual({ kind: "changed" });
    });
});

describe("checkAnchors", () => {
    test("fails an unchanged anchor in an edited document when its cited lines move", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "intro\nCheck: `src/a.rs:20-21` holds.\nnew line\n",
                "src/a.rs": SOURCE,
            },
            { "docs/p/catalog.md": modified("docs/p/catalog.md"), "src/a.rs": modified("src/a.rs") },
            {
                "docs/p/catalog.md": parseHunks("@@ -2,0 +3 @@\n"),
                "src/a.rs": parseHunks("@@ -1,0 +2,2 @@\n"),
            },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/p/catalog.md:2: src/a.rs:20-21 cites lines that moved to src/a.rs:22-23",
        ]);
    });

    test("reports the same drift in an untouched document as a notice", () => {
        const repo = repository(
            { "docs/p/catalog.md": "Check: `src/a.rs:20`.\n", "src/a.rs": SOURCE },
            { "src/a.rs": modified("src/a.rs") },
            { "src/a.rs": parseHunks("@@ -19,2 +19,2 @@\n") },
        );
        const report = checkAnchors(repo);
        expect(report.findings).toEqual([]);
        expect(messages(report.notices)).toEqual(["docs/p/catalog.md:1: src/a.rs:20 cites lines this change edits"]);
    });

    test("checks the later ranges of a comma-separated anchor", () => {
        const repo = repository(
            { "docs/p/catalog.md": "`src/a.rs:3-4,20-21` holds.\n", "src/a.rs": SOURCE },
            { "src/a.rs": modified("src/a.rs") },
            { "src/a.rs": parseHunks("@@ -10,0 +11,2 @@\n") },
        );
        expect(messages(checkAnchors(repo).notices)).toEqual([
            "docs/p/catalog.md:1: src/a.rs:20-21 cites lines that moved to src/a.rs:22-23",
        ]);
    });

    test("resolves a relative bare anchor and a reference definition from the document", () => {
        const repo = repository(
            {
                "docs/a/b/c.md": "intro\n`../../x.md:20` and `../../gone.md:2`\n[ref]: ../../../src/a.rs#L20\n",
                "docs/x.md": SOURCE,
                "src/a.rs": SOURCE,
                "other/x.md": SOURCE,
            },
            { "docs/a/b/c.md": modified("docs/a/b/c.md"), "docs/x.md": modified("docs/x.md"), "src/a.rs": modified("src/a.rs") },
            {
                "docs/a/b/c.md": parseHunks("@@ -0,0 +1 @@\n"),
                "docs/x.md": parseHunks("@@ -1,0 +2,2 @@\n"),
                "src/a.rs": parseHunks("@@ -1,0 +2,3 @@\n"),
            },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/a/b/c.md:2: ../../x.md:20 cites lines that moved to docs/x.md:22-22",
            "docs/a/b/c.md:3: [ref]: ../../../src/a.rs#L20 cites lines that moved to src/a.rs:23-23",
        ]);
    });

    test("fails an added relative bare anchor that resolves to no tracked file", () => {
        const repo = repository(
            { "docs/a/c.md": "`../gone.md:2` and `gone.md:2`\n", "docs/x.md": SOURCE },
            { "docs/a/c.md": { kind: "added" } },
            {},
        );
        expect(messages(checkAnchors(repo).findings)).toEqual(["docs/a/c.md:1: ../gone.md:2 cites no tracked file"]);
    });

    test("maps an anchor kept on an edited document line through the cited file's hunks", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "intro\nCheck now: `src/a.rs:20` holds, and `src/a.rs:5` is new.\n",
                "src/a.rs": SOURCE,
            },
            { "docs/p/catalog.md": modified("docs/p/catalog.md"), "src/a.rs": modified("src/a.rs") },
            {
                "docs/p/catalog.md": parseHunks("@@ -2 +2 @@\n"),
                "src/a.rs": parseHunks("@@ -1,0 +2,2 @@\n"),
            },
            {},
            { "docs/p/catalog.md": "intro\nCheck: `src/a.rs:20` holds.\n" },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/p/catalog.md:2: src/a.rs:20 cites lines that moved to src/a.rs:22-22",
        ]);
    });

    test("fails an added anchor past the end of its file and an added link to a missing file", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "[x](../../src/a.rs#L39-L41)\n[y](../../src/gone.rs#L1)\n",
                "src/a.rs": SOURCE,
            },
            { "docs/p/catalog.md": { kind: "added" } },
            {},
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
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
            { "docs/p/catalog.md": modified("docs/p/catalog.md") },
            { "docs/p/catalog.md": parseHunks("@@ -2,0 +2 @@\n") },
        );
        expect(checkAnchors(repo)).toEqual({ findings: [], notices: [] });
    });

    test("fails unchanged anchors in an edited document that cite a renamed or deleted file", () => {
        const repo = repository(
            {
                "docs/p/catalog.md": "intro\n`src/old.rs:20`\n`src/gone.rs:3`\nnew\n",
                "src/new.rs": SOURCE,
            },
            { "docs/p/catalog.md": modified("docs/p/catalog.md"), "src/new.rs": modified("src/old.rs") },
            { "docs/p/catalog.md": parseHunks("@@ -3,0 +4 @@\n"), "src/new.rs": parseHunks("@@ -1,0 +2 @@\n") },
            { "src/old.rs": "src/new.rs", "src/gone.rs": null },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/p/catalog.md:2: src/old.rs:20 cites lines that moved to src/new.rs:21-21",
            "docs/p/catalog.md:3: src/gone.rs:3 cites a file this change deletes",
        ]);
    });
});

describe("checkAnchors on a moved document", () => {
    test("reports a link that resolves to a different tracked file from the document's new path", () => {
        const repo = repository(
            { "docs/new/sub/a.md": "[x](../x.rs#L1)\n[y](../y.rs#L1)\n", "docs/x.rs": SOURCE, "docs/new/x.rs": SOURCE, "docs/y.rs": SOURCE },
            { "docs/new/sub/a.md": modified("docs/old/a.md") },
            {},
            { "docs/old/a.md": "docs/new/sub/a.md" },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/new/sub/a.md:1: ](../x.rs#L1) cites docs/new/x.rs from the document's new path; the target is docs/x.rs",
            "docs/new/sub/a.md:2: ](../y.rs#L1) cites no tracked file from the document's new path; the target is docs/y.rs",
        ]);
    });

    test("points a moved document's link at the cited file's current path, or reports its deletion", () => {
        const repo = repository(
            { "docs/c.md": "[r](../../src/old.rs#L2)\n[g](../../src/gone.rs#L2)\n", "src/new.rs": SOURCE },
            { "docs/c.md": modified("docs/old/c.md"), "src/new.rs": modified("src/old.rs") },
            {},
            { "docs/old/c.md": "docs/c.md", "src/old.rs": "src/new.rs", "src/gone.rs": null },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/c.md:1: ](../../src/old.rs#L2) cites no tracked file from the document's new path; the target is src/new.rs",
            "docs/c.md:2: ](../../src/gone.rs#L2) cites a file this change deletes",
        ]);
    });

    test("a rename within one directory reports a deleted link target as deleted", () => {
        const repo = repository(
            { "docs/b.md": "[x](../src/x.rs#L3)\n" },
            { "docs/b.md": modified("docs/a.md") },
            {},
            { "docs/a.md": "docs/b.md", "src/x.rs": null },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual(["docs/b.md:1: ](../src/x.rs#L3) cites a file this change deletes"]);
    });

    test("a rename within one directory reports a renamed link target as a renamed file", () => {
        const repo = repository(
            { "docs/b.md": "[x](../src/x.rs#L3)\n", "src/y.rs": SOURCE },
            { "docs/b.md": modified("docs/a.md"), "src/y.rs": modified("src/x.rs") },
            {},
            { "docs/a.md": "docs/b.md", "src/x.rs": "src/y.rs" },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/b.md:1: ](../src/x.rs#L3) cites a file this change renames to src/y.rs",
        ]);
    });
});

describe("checkAnchors on a renamed target", () => {
    test("accepts an unchanged anchor that already names the renamed file's new path", () => {
        const repo = repository(
            { "docs/a.md": "`src/x.rs:5`\nnew\n", "crates/b/src/x.rs": SOURCE },
            { "docs/a.md": modified("docs/a.md"), "crates/b/src/x.rs": modified("crates/a/src/x.rs") },
            { "docs/a.md": parseHunks("@@ -1,0 +2 @@\n") },
            { "crates/a/src/x.rs": "crates/b/src/x.rs" },
        );
        expect(checkAnchors(repo)).toEqual({ findings: [], notices: [] });
    });

    test("reports the file a bare anchor cited at the base when an added file takes over its name", () => {
        const repo = repository(
            {
                "docs/a.md": "intro\n`x.rs:3`, `y.rs:4`, `w.rs:20`, `v.rs:6`\nnew\n",
                "new/x.rs": SOURCE,
                "new/y.rs": SOURCE,
                "b/z.rs": SOURCE,
                "a/w.rs": SOURCE,
                "c/w.rs": SOURCE,
                "v.rs": SOURCE,
                "a/v.rs": SOURCE,
            },
            {
                "docs/a.md": modified("docs/a.md"),
                "new/x.rs": { kind: "added" },
                "new/y.rs": { kind: "added" },
                "b/z.rs": modified("old/y.rs"),
                "a/w.rs": modified("a/w.rs"),
                "c/w.rs": { kind: "added" },
                "v.rs": { kind: "added" },
            },
            { "docs/a.md": parseHunks("@@ -2,0 +3 @@\n"), "a/w.rs": parseHunks("@@ -1,0 +2 @@\n") },
            { "old/x.rs": null, "old/y.rs": "b/z.rs" },
        );
        expect(messages(checkAnchors(repo).findings)).toEqual([
            "docs/a.md:2: x.rs:3 cites a file this change deletes",
            "docs/a.md:2: y.rs:4 cites a file this change renames to b/z.rs",
            "docs/a.md:2: w.rs:20 matches several files after this change; the cited file is a/w.rs",
            "docs/a.md:2: v.rs:6 now resolves to v.rs; the cited file is a/v.rs",
        ]);
    });
});

describe("gitRepository", () => {
    let repo: string;

    function git(...args: string[]): string {
        const result = spawnSync("git", args, { cwd: repo, encoding: "utf8", env: ISOLATED_GIT });
        if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
        return result.stdout.trim();
    }

    function write(path: string, content: string): void {
        mkdirSync(dirname(join(repo, path)), { recursive: true });
        writeFileSync(join(repo, path), content);
    }

    beforeEach(() => {
        repo = mkdtempSync(join(tmpdir(), "check-doc-anchors-"));
        git("init", "-q", "-b", "main");
        git("config", "user.email", "test@example.com");
        git("config", "user.name", "test");
    });

    afterEach(() => {
        rmSync(repo, { recursive: true, force: true });
    });

    test("reads renames, edits, and added lines from a real history", () => {
        write("src/old.rs", SOURCE);
        write("src/b.rs", SOURCE);
        write("docs/a.md", "`src/old.rs:30` and `src/b.rs:30`\n");
        git("add", ".");
        git("commit", "-q", "-m", "base");
        const base = git("rev-parse", "HEAD");
        git("mv", "src/old.rs", "src/new.rs");
        write("src/b.rs", `inserted\n${SOURCE}`);
        write("docs/a.md", "`src/old.rs:30` and `src/b.rs:30` still\n[x](../src/new.rs#L41)\n");
        git("add", ".");
        git("commit", "-q", "-m", "change");
        expect(messages(checkAnchors(gitRepository(base, repo)).findings)).toEqual([
            "docs/a.md:1: src/old.rs:30 cites a file this change renames to src/new.rs",
            "docs/a.md:1: src/b.rs:30 cites lines that moved to src/b.rs:31-31",
            "docs/a.md:2: ](../src/new.rs#L41) cites past the end of src/new.rs (40 lines)",
        ]);
    });

    test("reads deletions, added documents, and a renamed document's relative links", () => {
        write("src/gone.rs", SOURCE);
        write("src/b.rs", SOURCE);
        write("docs/a.md", "`src/gone.rs:3`\nintro\n");
        write("docs/old/c.md", "[b](../../src/b.rs#L2)\n");
        git("add", ".");
        git("commit", "-q", "-m", "base");
        const base = git("rev-parse", "HEAD");
        git("rm", "-q", "src/gone.rs");
        write("docs/a.md", "`src/gone.rs:3`\nintro\nmore\n");
        write("docs/new.md", "[y](../src/b.rs#L99)\n");
        git("mv", "docs/old/c.md", "docs/c.md");
        git("add", ".");
        git("commit", "-q", "-m", "change");
        expect(messages(checkAnchors(gitRepository(base, repo)).findings)).toEqual([
            "docs/a.md:1: src/gone.rs:3 cites a file this change deletes",
            "docs/c.md:1: ](../../src/b.rs#L2) cites no tracked file from the document's new path; the target is src/b.rs",
            "docs/new.md:1: ](../src/b.rs#L99) cites past the end of src/b.rs (40 lines)",
        ]);
    });
});
