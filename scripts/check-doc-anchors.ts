// Usage: `bun scripts/check-doc-anchors.ts <base-ref>`.

export interface Anchor {
    line: number;
    text: string;
    path: string;
    /** `relative` paths resolve from the document's directory. */
    form: "relative" | "bare";
    start: number;
    end: number;
}

const LINK = /\]\(([^)\s#]+)#L(\d+)(?:-L?(\d+))?\)/g;
const DEFINITION = /^\s*\[[^\]]+\]:\s*(\S+?)#L(\d+)(?:-L?(\d+))?\s*$/;
const RANGES = String.raw`\d+(?:-\d+)?(?:,\d+(?:-\d+)?)*`;
const BARE = new RegExp(
    String.raw`(?<![\w/.-])((?:[\w.-]+\/)*[\w.-]+\.(?:rs|ts|tsx|js|mjs|sql|toml|ya?ml|sh|json|md|py|lock|tiktoken)):(${RANGES})(?![\w-]|\.\d)|` +
        String.raw`\x60:(${RANGES})\x60`,
    "g",
);
const SCHEME = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

/**
 * A backticked range such as `` `:2045-2064` `` inherits the path of the last bare anchor with an
 * explicit path on the same or preceding line. Blank lines and table rows end that reach.
 */
export function findAnchors(text: string): Anchor[] {
    const anchors: Anchor[] = [];
    let previous: string | undefined;
    let previousLine = 0;
    text.split("\n").forEach((content, index) => {
        const line = index + 1;
        if (content.trim() === "" || content.startsWith("|") || previousLine < line - 1) previous = undefined;
        const definition = content.match(DEFINITION);
        const links = definition === null ? [...content.matchAll(LINK)] : [definition];
        let plain = content;
        for (const match of links) {
            plain = plain.replace(match[0], "");
            const path = match[1] ?? "";
            if (SCHEME.test(path)) continue;
            const start = Number(match[2]);
            anchors.push({ line, text: match[0].trim(), path, form: "relative", start, end: Number(match[3] ?? start) });
        }
        for (const match of plain.matchAll(BARE)) {
            const path = match[1] ?? previous;
            if (path === undefined) continue;
            if (match[1] !== undefined) {
                previous = path;
                previousLine = line;
            }
            const form = path.startsWith("./") || path.startsWith("../") ? "relative" : "bare";
            for (const range of (match[2] ?? match[3] ?? "").split(",")) {
                const [first, second] = range.split("-");
                const start = Number(first);
                anchors.push({ line, text: `${path}:${range}`, path, form, start, end: Number(second ?? first) });
            }
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

function hunkLines(hunks: Hunk[], side: "old" | "new"): Set<number> {
    const lines = new Set<number>();
    for (const hunk of hunks) {
        const [start, count] = side === "old" ? [hunk.oldStart, hunk.oldCount] : [hunk.newStart, hunk.newCount];
        for (let offset = 0; offset < count; offset++) lines.add(start + offset);
    }
    return lines;
}

/**
 * An anchor that reappears on a replacement line is an existing citation whose surrounding prose
 * changed, so it still maps through its target's hunks.
 */
function removedAnchors(baseText: string, hunks: Hunk[]): Set<string> {
    const removed = hunkLines(hunks, "old");
    return new Set(findAnchors(baseText).filter((anchor) => removed.has(anchor.line)).map((anchor) => anchor.text));
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
    baseText(path: string): string;
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
    if (anchor.form === "relative") {
        const path = normalize(`${dirname(doc)}/${anchor.path}`);
        return path !== undefined && paths.has(path) ? path : undefined;
    }
    const matches = bareMatches(paths, anchor.path);
    return matches.length === 1 ? matches[0] : undefined;
}

function bareMatches(paths: Set<string>, path: string): string[] {
    if (paths.has(path)) return [path];
    const suffix = `/${path}`;
    return [...paths].filter((candidate) => candidate.endsWith(suffix));
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
    const baseTracked = new Set(repo.removed.keys());
    for (const path of repo.tracked) {
        const change = repo.changes.get(path);
        if (change === undefined || (change.kind === "modified" && change.base === path)) baseTracked.add(path);
    }
    const docs = [...repo.tracked].filter((path) => path.endsWith(".md")).sort();
    for (const doc of docs) {
        const docChange = repo.changes.get(doc);
        const docAdded = docChange?.kind === "added";
        const docHunks = docChange?.kind === "modified" ? repo.hunks(docChange.base, doc) : [];
        const added = hunkLines(docHunks, "new");
        const kept =
            docChange?.kind === "modified" && docHunks.some((hunk) => hunk.oldCount > 0)
                ? removedAnchors(repo.baseText(docChange.base), docHunks)
                : new Set<string>();
        const entries = docChange === undefined ? report.notices : report.findings;
        for (const anchor of findAnchors(repo.text(doc))) {
            const push = (sink: Entry[], message: string) =>
                sink.push({ doc, line: anchor.line, message: `${anchor.text} ${message}` });
            const target = resolveAnchor(repo.tracked, doc, anchor);
            const outOfBounds = (file: string): string | undefined => {
                if (anchor.start < 1 || anchor.end < anchor.start) return "cites an empty or reversed range";
                const count = lineCount(repo.text(file));
                return anchor.end > count ? `cites past the end of ${file} (${count} lines)` : undefined;
            };
            const edited = added.has(anchor.line);
            if (docAdded || (edited && !kept.has(anchor.text))) {
                if (target === undefined) {
                    if (anchor.form === "relative") push(report.findings, "cites no tracked file");
                    continue;
                }
                const message = outOfBounds(target);
                if (message !== undefined) push(report.findings, message);
                continue;
            }
            const located = locate(repo, baseTracked, doc, docChange, anchor, target);
            if (located === undefined) continue;
            if (located === "deleted") {
                push(entries, "cites a file this change deletes");
                continue;
            }
            if (located.kind === "relink") {
                const cites = target === undefined ? "cites no tracked file" : `cites ${target}`;
                push(entries, `${cites} from the document's new path; the target is ${located.path}`);
                continue;
            }
            if (located.kind === "retargeted") {
                const now = target === undefined ? "matches several files after this change" : `now resolves to ${target}`;
                push(entries, `${now}; the cited file is ${located.path}`);
                continue;
            }
            const { base, head } = located;
            const hunks = repo.hunks(base, head);
            let mapped = mapRange(hunks, anchor.start, anchor.end);
            const renamedTo = located.kind === "renamed" ? ` in ${head}` : "";
            if (mapped.kind === "changed" && edited && located.kind === "modified") {
                // An anchor kept on an edited document line is reaffirmed: an in-place edit of its
                // cited lines passes while the range exists at HEAD. Hunks above the range still
                // shift it, and that shift reports as moved. A pure insertion names the old line it
                // follows, so one at `start - 1` is above the range and one at `start` is inside it.
                const above = hunks.filter((hunk) => Math.max(hunk.oldStart, hunk.oldStart + hunk.oldCount - 1) < anchor.start);
                mapped = mapRange(above, anchor.start, anchor.end);
                if (mapped.kind === "same") {
                    const message = outOfBounds(head);
                    if (message !== undefined) push(entries, message);
                }
            }
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
    | { kind: "retargeted"; path: string }
    | "deleted"
    | undefined;

/**
 * `baseTracked` holds the paths that existed at the merge base. `locate` resolves the anchor
 * against them first and follows that file through renames and deletions, so it reports the
 * deletion even when a file this change adds reuses the name.
 */
function locate(
    repo: Repository,
    baseTracked: Set<string>,
    doc: string,
    docChange: FileChange | undefined,
    anchor: Anchor,
    target: string | undefined,
): Located {
    if (anchor.form === "relative" && docChange?.kind === "modified" && dirname(docChange.base) !== dirname(doc)) {
        const path = normalize(`${dirname(docChange.base)}/${anchor.path}`);
        const cited = path === undefined ? undefined : repo.tracked.has(path) ? path : repo.removed.get(path);
        if (cited === null) return "deleted";
        if (cited !== undefined && cited !== target) return { kind: "relink", path: cited };
    }
    const meant = resolveAnchor(baseTracked, doc, anchor);
    if (meant === undefined) {
        const change = target === undefined ? undefined : repo.changes.get(target);
        return change?.kind === "modified" && target !== undefined ? { kind: "modified", base: change.base, head: target } : undefined;
    }
    const now = repo.removed.has(meant) ? repo.removed.get(meant) ?? null : meant;
    if (now === null) return "deleted";
    if (target !== now && now === meant) return { kind: "retargeted", path: now };
    const change = repo.changes.get(now);
    const kind = now === target ? "modified" : "renamed";
    if (change?.kind === "modified") return { kind, base: change.base, head: now };
    return kind === "renamed" ? { kind, base: meant, head: now } : undefined;
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
        if (code.startsWith("R")) {
            const headPath = status[index + 2] ?? "";
            changes.set(headPath, { kind: "modified", base: path });
            removed.set(path, headPath);
            index += 3;
            continue;
        }
        if (code.startsWith("D")) removed.set(path, null);
        else if (code.startsWith("A")) changes.set(path, { kind: "added" });
        else changes.set(path, { kind: "modified", base: path });
        index += 2;
    }
    const texts = new Map<string, string>();
    const show = (revision: string, path: string): string => {
        const key = `${revision}:${path}`;
        let content = texts.get(key);
        if (content === undefined) {
            content = git(cwd, ["show", key]);
            texts.set(key, content);
        }
        return content;
    };
    const hunkCache = new Map<string, Hunk[]>();
    return {
        tracked,
        changes,
        removed,
        text: (path) => show("HEAD", path),
        baseText: (path) => show(mergeBase, path),
        hunks: (basePath, headPath) => {
            const key = `${basePath}\0${headPath}`;
            let hunks = hunkCache.get(key);
            if (hunks === undefined) {
                hunks = parseHunks(
                    git(cwd, ["diff", "-U0", "--no-color", "--no-ext-diff", `${mergeBase}:${basePath}`, `HEAD:${headPath}`]),
                );
                hunkCache.set(key, hunks);
            }
            return hunks;
        },
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
