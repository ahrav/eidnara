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
    /(?<![\w/.-])((?:[\w.-]+\/)*[\w.-]+\.(?:rs|ts|tsx|js|mjs|sql|toml|ya?ml|sh|json)):(\d+)(?:-(\d+))?(?![\w-]|\.\d)/g;

export function findAnchors(text: string): Anchor[] {
    const anchors: Anchor[] = [];
    text.split("\n").forEach((content, index) => {
        let plain = content;
        for (const match of content.matchAll(LINK)) {
            const start = Number(match[2]);
            anchors.push({
                line: index + 1,
                text: match[0],
                path: match[1] ?? "",
                form: "link",
                start,
                end: Number(match[3] ?? start),
            });
            plain = plain.replace(match[0], "");
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
 * survives unchanged, `changed` when a hunk modifies or deletes any of them.
 */
export function mapRange(hunks: Hunk[], start: number, end: number): Mapped {
    let shift = 0;
    for (const hunk of hunks) {
        if (hunk.oldCount > 0) {
            const last = hunk.oldStart + hunk.oldCount - 1;
            if (hunk.oldStart <= end && last >= start) return { kind: "changed" };
            if (last < start) shift += hunk.newCount - hunk.oldCount;
        } else if (hunk.oldStart < start) {
            // A pure insertion follows old line `oldStart`.
            shift += hunk.newCount;
        }
    }
    return shift === 0 ? { kind: "same" } : { kind: "moved", start: start + shift, end: end + shift };
}

export interface Repository {
    tracked: Set<string>;
    text(path: string): string;
    changed: Map<string, string>;
    hunks(basePath: string, headPath: string): Hunk[];
}

function normalize(path: string): string {
    const parts: string[] = [];
    for (const part of path.split("/")) {
        if (part === "..") parts.pop();
        else if (part !== "." && part !== "") parts.push(part);
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

export function resolveAnchor(tracked: Set<string>, doc: string, anchor: Anchor): string | undefined {
    if (anchor.form === "link") {
        const path = normalize(`${dirname(doc)}/${anchor.path}`);
        return tracked.has(path) ? path : undefined;
    }
    if (tracked.has(anchor.path)) return anchor.path;
    const suffix = `/${anchor.path}`;
    const matches = [...tracked].filter((path) => path.endsWith(suffix));
    return matches.length === 1 ? matches[0] : undefined;
}

export interface Report {
    findings: string[];
    notices: string[];
}

export function checkAnchors(repo: Repository): Report {
    const report: Report = { findings: [], notices: [] };
    const hunkCache = new Map<string, Hunk[]>();
    const hunksOf = (path: string): Hunk[] => {
        let hunks = hunkCache.get(path);
        if (hunks === undefined) {
            const base = repo.changed.get(path) ?? path;
            hunks = base === "" ? [] : repo.hunks(base, path);
            hunkCache.set(path, hunks);
        }
        return hunks;
    };
    const docs = [...repo.tracked].filter((path) => path.endsWith(".md")).sort();
    for (const doc of docs) {
        const docBase = repo.changed.get(doc);
        const docChanged = docBase !== undefined;
        const docAdded = docBase === "";
        const added = docChanged && !docAdded ? addedLines(hunksOf(doc)) : new Set<number>();
        for (const anchor of findAnchors(repo.text(doc))) {
            const where = `${doc}:${anchor.line}: ${anchor.text}`;
            const target = resolveAnchor(repo.tracked, doc, anchor);
            if (docAdded || added.has(anchor.line)) {
                if (target === undefined) {
                    if (anchor.form === "link") report.findings.push(`${where} cites no tracked file`);
                    continue;
                }
                const count = lineCount(repo.text(target));
                if (anchor.start < 1 || anchor.end < anchor.start || anchor.end > count) {
                    report.findings.push(`${where} cites past the end of ${target} (${count} lines)`);
                }
                continue;
            }
            if (target === undefined) continue;
            const targetBase = repo.changed.get(target);
            if (targetBase === undefined || targetBase === "") continue;
            const mapped = mapRange(hunksOf(target), anchor.start, anchor.end);
            if (mapped.kind === "same") continue;
            const message =
                mapped.kind === "moved"
                    ? `${where} cites lines that moved to ${mapped.start}-${mapped.end}`
                    : `${where} cites lines this change edits`;
            (docChanged ? report.findings : report.notices).push(message);
        }
    }
    return report;
}

function git(args: string[]): string {
    const result = Bun.spawnSync(["git", ...args], { stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) {
        throw new Error(`git ${args.join(" ")} exited ${result.exitCode}: ${result.stderr.toString()}`);
    }
    return result.stdout.toString();
}

export function gitRepository(base: string): Repository {
    const mergeBase = git(["merge-base", base, "HEAD"]).trim();
    const tracked = new Set(git(["ls-tree", "-r", "-z", "--name-only", "HEAD"]).split("\0").filter(Boolean));
    const changed = new Map<string, string>();
    const status = git(["diff", "--name-status", "-z", "-M", mergeBase, "HEAD"]).split("\0");
    for (let index = 0; index + 1 < status.length; ) {
        const code = status[index] ?? "";
        if (code.startsWith("R") || code.startsWith("C")) {
            changed.set(status[index + 2] ?? "", status[index + 1] ?? "");
            index += 3;
        } else {
            const path = status[index + 1] ?? "";
            if (!code.startsWith("D")) changed.set(path, code.startsWith("A") ? "" : path);
            index += 2;
        }
    }
    const texts = new Map<string, string>();
    return {
        tracked,
        changed,
        text: (path) => {
            let content = texts.get(path);
            if (content === undefined) {
                content = git(["show", `HEAD:${path}`]);
                texts.set(path, content);
            }
            return content;
        },
        hunks: (basePath, headPath) =>
            parseHunks(git(["diff", "-U0", "--no-color", "--no-ext-diff", `${mergeBase}:${basePath}`, `HEAD:${headPath}`])),
    };
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
    for (const notice of report.notices) console.log(`notice: ${notice}`);
    if (report.findings.length > 0) {
        console.error("line anchors in changed Markdown need updating:");
        for (const finding of report.findings) console.error(`  ${finding}`);
        process.exit(1);
    }
    console.log(`doc anchors: changed Markdown cites current lines (${report.notices.length} notices)`);
}
