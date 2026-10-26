// Usage: `bun scripts/check-doc-anchors.ts <base-ref>`.

export interface Anchor {
    line: number;
    text: string;
    path: string;
    form: "link" | "bare";
    start: number;
    end: number;
}

const LINK = /\]\(([^)\s#]+)#L(\d+)(?:-L?(\d+))?\)/g;
const BARE =
    /(?<![\w/.-])((?:[\w.-]+\/)*[\w.-]+\.(?:rs|ts|tsx|js|mjs|sql|toml|ya?ml|sh|json|md|py)):(\d+)(?:-(\d+))?(?![\w-]|\.\d)/g;
const SCHEME = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

export function findAnchors(text: string): Anchor[] {
    const anchors: Anchor[] = [];
    text.split("\n").forEach((content, index) => {
        let plain = content;
        for (const match of content.matchAll(LINK)) {
            plain = plain.replace(match[0], "");
            const path = match[1] ?? "";
            if (SCHEME.test(path)) continue;
            const start = Number(match[2]);
            anchors.push({ line: index + 1, text: match[0], path, form: "link", start, end: Number(match[3] ?? start) });
        }
        for (const match of plain.matchAll(BARE)) {
            const start = Number(match[2]);
            anchors.push({
                line: index + 1,
                text: match[0],
                path: match[1] ?? "",
                form: "bare",
                start,
                end: Number(match[3] ?? start),
            });
        }
    });
    return anchors;
}

export interface Hunk {
    oldStart: number;
    oldCount: number;
    newStart: number;
    newCount: number;
}

export function parseHunks(diff: string): Hunk[] {
    const hunks: Hunk[] = [];
    for (const match of diff.matchAll(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/gm)) {
        hunks.push({
            oldStart: Number(match[1]),
            oldCount: Number(match[2] ?? 1),
            newStart: Number(match[3]),
            newCount: Number(match[4] ?? 1),
        });
    }
    return hunks;
}

export function addedLines(hunks: Hunk[]): Set<number> {
    const added = new Set<number>();
    for (const hunk of hunks) {
        for (let offset = 0; offset < hunk.newCount; offset++) added.add(hunk.newStart + offset);
    }
    return added;
}

export type Mapped = { kind: "same" } | { kind: "moved"; start: number; end: number } | { kind: "changed" };

/**
 * Where the old-side lines `start..end` sit on the new side: `same` or `moved` when every line
 * survives unchanged and stays contiguous, `changed` when a hunk modifies, deletes, or inserts
 * inside them.
 */
export function mapRange(hunks: Hunk[], start: number, end: number): Mapped {
    let shift = 0;
    for (const hunk of hunks) {
        if (hunk.oldCount > 0) {
            const last = hunk.oldStart + hunk.oldCount - 1;
            if (hunk.oldStart <= end && last >= start) return { kind: "changed" };
            if (last < start) shift += hunk.newCount - hunk.oldCount;
        } else if (hunk.oldStart >= start && hunk.oldStart < end) {
            return { kind: "changed" };
        } else if (hunk.oldStart < start) {
            shift += hunk.newCount;
        }
    }
    return shift === 0 ? { kind: "same" } : { kind: "moved", start: start + shift, end: end + shift };
}

export type FileChange = { kind: "added" } | { kind: "modified"; base: string };

export interface Repository {
    tracked: Set<string>;
    text(path: string): string;
    /** `changes` maps HEAD paths to file changes. */
    changes: Map<string, FileChange>;
    /** `removed` maps a base path to its HEAD path after a rename, or to `null` after a deletion. */
    removed: Map<string, string | null>;
    hunks(basePath: string, headPath: string): Hunk[];
}

function normalize(path: string): string | undefined {
    const parts: string[] = [];
    for (const part of path.split("/")) {
        if (part === "..") {
            if (parts.length === 0) return undefined;
            parts.pop();
        } else if (part !== "." && part !== "") parts.push(part);
    }
    return parts.join("/");
}

function dirname(path: string): string {
    const slash = path.lastIndexOf("/");
    return slash < 0 ? "" : path.slice(0, slash);
}

function lineCount(text: string): number {
    return text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
}

export function resolveAnchor(paths: Set<string>, doc: string, anchor: Anchor): string | undefined {
    if (anchor.form === "link") {
        const path = normalize(`${dirname(doc)}/${anchor.path}`);
        return path !== undefined && paths.has(path) ? path : undefined;
    }
    if (paths.has(anchor.path)) return anchor.path;
    const suffix = `/${anchor.path}`;
    const matches = [...paths].filter((path) => path.endsWith(suffix));
    return matches.length === 1 ? matches[0] : undefined;
}

export interface Entry {
    doc: string;
    line: number;
    message: string;
}

export interface Report {
    findings: Entry[];
    notices: Entry[];
}

export function checkAnchors(repo: Repository): Report {
    const report: Report = { findings: [], notices: [] };
    const hunkCache = new Map<string, Hunk[]>();
    const hunksOf = (base: string, head: string): Hunk[] => {
        const key = `${base}\0${head}`;
        let hunks = hunkCache.get(key);
        if (hunks === undefined) {
            hunks = repo.hunks(base, head);
            hunkCache.set(key, hunks);
        }
        return hunks;
    };
    const removedPaths = new Set(repo.removed.keys());
    const docs = [...repo.tracked].filter((path) => path.endsWith(".md")).sort();
    for (const doc of docs) {
        const docChange = repo.changes.get(doc);
        const docAdded = docChange?.kind === "added";
        const added =
            docChange?.kind === "modified" ? addedLines(hunksOf(docChange.base, doc)) : new Set<number>();
        const entries = docChange === undefined ? report.notices : report.findings;
        for (const anchor of findAnchors(repo.text(doc))) {
            const push = (sink: Entry[], message: string) =>
                sink.push({ doc, line: anchor.line, message: `${anchor.text} ${message}` });
            const target = resolveAnchor(repo.tracked, doc, anchor);
            if (docAdded || added.has(anchor.line)) {
                if (target === undefined) {
                    if (anchor.form === "link") push(report.findings, "cites no tracked file");
                    continue;
                }
                const count = lineCount(repo.text(target));
                if (anchor.start < 1 || anchor.end < anchor.start || anchor.end > count) {
                    push(report.findings, `cites past the end of ${target} (${count} lines)`);
                }
                continue;
            }
            const located = locate(repo, removedPaths, doc, docChange, anchor, target);
            if (located === undefined) continue;
            if (located === "deleted") {
                push(entries, "cites a file this change deletes");
                continue;
            }
            if (located.kind === "relink") {
                push(entries, `cites no tracked file from the document's new path; the target is ${located.path}`);
                continue;
            }
            const { base, head } = located;
            const mapped = mapRange(hunksOf(base, head), anchor.start, anchor.end);
            const renamedTo = located.kind === "renamed" ? ` in ${head}` : "";
            if (mapped.kind === "changed") push(entries, `cites lines this change edits${renamedTo}`);
            else if (mapped.kind === "moved") push(entries, `cites lines that moved to ${head}:${mapped.start}-${mapped.end}`);
            else if (located.kind === "renamed") push(entries, `cites a file this change renames to ${head}`);
        }
    }
    return report;
}

type Located =
    | { kind: "modified" | "renamed"; base: string; head: string }
    | { kind: "relink"; path: string }
    | "deleted"
    | undefined;

function locate(
    repo: Repository,
    removedPaths: Set<string>,
    doc: string,
    docChange: FileChange | undefined,
    anchor: Anchor,
    target: string | undefined,
): Located {
    if (target !== undefined) {
        const change = repo.changes.get(target);
        return change?.kind === "modified" ? { kind: "modified", base: change.base, head: target } : undefined;
    }
    if (anchor.form === "link" && docChange?.kind === "modified" && docChange.base !== doc) {
        const path = normalize(`${dirname(docChange.base)}/${anchor.path}`);
        if (path !== undefined && (repo.tracked.has(path) || repo.removed.has(path))) return { kind: "relink", path };
    }
    const suffix = `/${anchor.path}`;
    if (anchor.form === "bare" && [...repo.tracked].some((path) => path.endsWith(suffix))) return undefined;
    const gone = resolveAnchor(removedPaths, doc, anchor);
    if (gone === undefined) return undefined;
    const renamed = repo.removed.get(gone) ?? null;
    return renamed === null ? "deleted" : { kind: "renamed", base: gone, head: renamed };
}

function git(cwd: string | undefined, args: string[]): string {
    const result = Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) {
        throw new Error(`git ${args.join(" ")} exited ${result.exitCode}: ${result.stderr.toString()}`);
    }
    return result.stdout.toString();
}

export function gitRepository(base: string, cwd?: string): Repository {
    const mergeBase = git(cwd, ["merge-base", base, "HEAD"]).trim();
    const tracked = new Set(git(cwd, ["ls-tree", "-r", "-z", "--name-only", "HEAD"]).split("\0").filter(Boolean));
    const changes = new Map<string, FileChange>();
    const removed = new Map<string, string | null>();
    const status = git(cwd, ["diff", "--name-status", "-z", "-M", mergeBase, "HEAD"]).split("\0");
    for (let index = 0; index + 1 < status.length; ) {
        const code = status[index] ?? "";
        const path = status[index + 1] ?? "";
        if (code.startsWith("R") || code.startsWith("C")) {
            const headPath = status[index + 2] ?? "";
            changes.set(headPath, { kind: "modified", base: path });
            if (code.startsWith("R")) removed.set(path, headPath);
            index += 3;
            continue;
        }
        if (code.startsWith("D")) removed.set(path, null);
        else if (code.startsWith("A")) changes.set(path, { kind: "added" });
        else changes.set(path, { kind: "modified", base: path });
        index += 2;
    }
    const texts = new Map<string, string>();
    return {
        tracked,
        changes,
        removed,
        text: (path) => {
            let content = texts.get(path);
            if (content === undefined) {
                content = git(cwd, ["show", `HEAD:${path}`]);
                texts.set(path, content);
            }
            return content;
        },
        hunks: (basePath, headPath) =>
            parseHunks(
                git(cwd, ["diff", "-U0", "--no-color", "--no-ext-diff", `${mergeBase}:${basePath}`, `HEAD:${headPath}`]),
            ),
    };
}

function annotation(kind: "error" | "notice", entry: Entry): string {
    const message = entry.message.replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A");
    return `::${kind} file=${entry.doc},line=${entry.line}::${message}`;
}

if (import.meta.main) {
    const base = process.argv[2];
    if (base === undefined) {
        console.error("usage: bun scripts/check-doc-anchors.ts <base-ref>");
        process.exit(2);
    }
    let report: Report;
    try {
        report = checkAnchors(gitRepository(base));
    } catch (error) {
        console.error(String(error));
        process.exit(2);
    }
    for (const notice of report.notices) console.log(annotation("notice", notice));
    if (report.findings.length > 0) {
        console.error("line anchors in changed Markdown need updating:");
        for (const finding of report.findings) console.log(annotation("error", finding));
        process.exit(1);
    }
    console.log(`doc anchors: changed Markdown cites current lines (${report.notices.length} notices)`);
}
