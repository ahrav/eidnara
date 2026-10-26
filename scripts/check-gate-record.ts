export interface Round {
    number: number;
    base: string;
    head: string;
}

const ROUND = /\bRound\s+(\d+)\s+reviewed\s+`?([^\s`]+?)`?\s*\.\.\s*`?([0-9a-f]{7,40})`?/gi;

export function lastRound(body: string): Round | undefined {
    let last: Round | undefined;
    for (const match of body.matchAll(ROUND)) {
        const round = { number: Number(match[1]), base: match[2] ?? "", head: match[3] ?? "" };
        if (last === undefined || round.number >= last.number) last = round;
    }
    return last;
}

// REVIEW.md gates production code and the Markdown contract, property, and
// test-evidence claims, which live under `docs/`; `docs/agents/` holds guidance.
export function needsGate(path: string): boolean {
    return !path.endsWith(".md") || (path.startsWith("docs/") && !path.startsWith("docs/agents/"));
}

export interface Change {
    files: string[];
    resolve(revision: string): string | undefined;
    isAncestor(ancestor: string, descendant: string): boolean;
    changedBetween(from: string, to: string): string[];
}

export function checkGateRecord(body: string, head: string, change: Change): string[] {
    const gated = change.files.filter(needsGate);
    if (gated.length === 0) return [];
    const round = lastRound(body);
    if (round === undefined) {
        return [
            `the pull request changes ${gated.length} gated files and its body records no ` +
                "`Round N reviewed <base>..<head>` line (REVIEW.md, Sub-agents)",
        ];
    }
    const reviewed = change.resolve(round.head);
    if (reviewed === undefined) {
        return [`Round ${round.number} names ${round.head}, which is not a commit in this repository`];
    }
    if (reviewed === head) return [];
    if (!change.isAncestor(reviewed, head)) {
        return [`Round ${round.number} reviewed ${round.head}, which is not an ancestor of the head ${head}`];
    }
    const files = new Set(change.files);
    const unreviewed = change
        .changedBetween(reviewed, head)
        .filter((path) => files.has(path) && needsGate(path))
        .sort();
    return [
        `Round ${round.number} reviewed ${round.head}, which is not the head ${head}; ` +
            "record a new round for the head (REVIEW.md, Sub-agents)",
        unreviewed.length === 0
            ? "  no gated file the pull request changes differs between them; state `no rerun - <reason>`"
            : "  rerun the reviews that cover these files:",
        ...unreviewed.map((path) => `  ${path}`),
    ];
}

function git(cwd: string | undefined, args: string[]): { code: number; out: string } {
    const result = Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
    return { code: result.exitCode, out: result.stdout.toString() };
}

function gitOk(cwd: string | undefined, args: string[]): string {
    const { code, out } = git(cwd, args);
    if (code !== 0) throw new Error(`git ${args.join(" ")} exited ${code}`);
    return out;
}

function names(out: string): string[] {
    return out.split("\0").filter(Boolean);
}

export function gitChange(base: string, head: string, cwd?: string): Change {
    const mergeBase = gitOk(cwd, ["merge-base", base, head]).trim();
    // `--no-renames` lists both paths of a rename, so the check sees a deleted source path.
    const diffNames = (from: string, to: string) =>
        names(gitOk(cwd, ["diff", "--name-only", "--no-renames", "-z", from, to]));
    return {
        files: diffNames(mergeBase, head),
        resolve: (revision) => {
            const { code, out } = git(cwd, ["rev-parse", "--verify", "--quiet", `${revision}^{commit}`]);
            return code === 0 ? out.trim() : undefined;
        },
        isAncestor: (ancestor, descendant) =>
            git(cwd, ["merge-base", "--is-ancestor", ancestor, descendant]).code === 0,
        changedBetween: diffNames,
    };
}

if (import.meta.main) {
    const [base, head] = process.argv.slice(2);
    if (base === undefined || head === undefined) {
        console.error("usage: PR_BODY=... bun scripts/check-gate-record.ts <base-sha> <head-sha>");
        process.exit(2);
    }
    let findings: string[];
    try {
        findings = checkGateRecord(process.env.PR_BODY ?? "", head, gitChange(base, head));
    } catch (error) {
        console.error(String(error));
        process.exit(2);
    }
    if (findings.length > 0) {
        for (const finding of findings) console.error(finding);
        process.exit(1);
    }
    console.log("gate record: the last recorded round names the head");
}
