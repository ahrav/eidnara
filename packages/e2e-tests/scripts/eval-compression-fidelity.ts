#!/usr/bin/env bun
/**
 * `eval:compression-fidelity`: assembles one baseline and one candidate evidence directory and
 * their review records into a private manifest and per-scenario report. Usage:
 *
 *   bun run eval:compression-fidelity --baseline <dir> --candidate <dir> --reviews <dir>
 *     --out <private dir> [--corpus <path>] [--mode offline|live]
 *
 * The command reads files and writes two files; it sends no request in either mode.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { publishPrivateJson, realDirectory } from "../src/atomic-publish";
import {
    COMPRESSION_FIDELITY_CORPUS_PATH,
    COMPRESSION_FIDELITY_CORPUS_SHA256,
    readCompressionFidelityCorpus,
} from "../src/compression-fidelity/corpus";
import { loadArm, sha256 } from "../src/compression-fidelity/evidence";
import { evaluate, loadReviews } from "../src/compression-fidelity/review";

export const USAGE =
    "eval:compression-fidelity --baseline <dir> --candidate <dir> --reviews <dir> --out <dir> [--corpus <path>] [--mode offline|live]";

export interface EvalArgs {
    baseline: string;
    candidate: string;
    reviews: string;
    out: string;
    corpus: string;
    mode: "offline" | "live";
}

export function parseArgs(argv: readonly string[]): EvalArgs {
    const values = new Map<string, string>();
    for (let i = 0; i < argv.length; i += 2) {
        const flag = argv[i];
        const value = argv[i + 1];
        if (!flag?.startsWith("--") || value === undefined || values.has(flag.slice(2))) {
            throw new Error(USAGE);
        }
        values.set(flag.slice(2), value);
    }
    const required = (name: string) => {
        const value = values.get(name);
        if (!value) throw new Error(`--${name} is required: ${USAGE}`);
        return value;
    };
    const mode = values.get("mode") ?? "offline";
    if (mode !== "offline" && mode !== "live")
        throw new Error(`--mode is offline or live: ${USAGE}`);
    for (const name of values.keys()) {
        if (!["baseline", "candidate", "reviews", "out", "corpus", "mode"].includes(name)) {
            throw new Error(`unknown flag --${name}: ${USAGE}`);
        }
    }
    return {
        baseline: required("baseline"),
        candidate: required("candidate"),
        reviews: required("reviews"),
        out: required("out"),
        corpus: values.get("corpus") ?? COMPRESSION_FIDELITY_CORPUS_PATH,
        mode,
    };
}

/** Runs the assembly and returns the written paths and whether the comparison was accepted. */
export async function run(
    args: EvalArgs,
): Promise<{ manifest: string; report: string; accepted: boolean }> {
    const out = realDirectory(args.out);
    if ([args.baseline, args.candidate].some((arm) => realDirectory(arm) === out)) {
        throw new Error(`--out is an evidence arm: ${out}`);
    }
    const revision = repositoryRevision();
    const corpus = readCompressionFidelityCorpus(args.corpus);
    const sha = COMPRESSION_FIDELITY_CORPUS_SHA256;
    const [baseline, candidate, reviews] = await Promise.all([
        loadArm(args.baseline, corpus, sha),
        loadArm(args.candidate, corpus, sha),
        loadReviews(args.reviews, sha),
    ]);
    const { manifest, report } = evaluate({
        corpus,
        corpusPath: args.corpus,
        corpusSha256: sha,
        revision: await revision,
        mode: args.mode,
        baseline,
        candidate,
        reviews,
    });
    // The report names the manifest bytes it was assembled with, so a report left beside an
    // older or newer manifest is detectable.
    const manifestPath = publishPrivateJson(manifest, out, "manifest.json", { checked: true });
    const manifestSha256 = sha256(readFileSync(manifestPath));
    return {
        manifest: manifestPath,
        report: publishPrivateJson(
            { ...report, manifest_sha256: manifestSha256 },
            out,
            "report.json",
            { checked: true },
        ),
        accepted: report.accepted === true,
    };
}

/** Environment variables that override or bound git's repository discovery from `cwd`. */
const DISCOVERY_ENV = ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_CEILING_DIRECTORIES"];
const OBJECT_ID = /^[0-9a-f]{40}([0-9a-f]{24})?$/;

/**
 * The commit `git rev-parse HEAD` names in `cwd`'s repository, or `"unknown"` outside one. The
 * reader follows HEAD through the repository's `.git` entry, a linked worktree's common
 * directory, and loose or packed branch refs. Any other layout runs `git rev-parse HEAD`.
 */
export async function repositoryRevision(cwd = process.cwd()): Promise<string> {
    const head = headFromFiles(cwd) ?? (await gitRevParse(cwd));
    if (head === "unknown") return head;
    const dirty = await worktreeDirty(cwd);
    return dirty === null ? "unknown" : `${head}${dirty ? "-dirty" : ""}`;
}

/**
 * Whether `cwd`'s worktree holds uncommitted or untracked changes, so the manifest never
 * attributes a report to code that did not produce it; `null` when git cannot say.
 */
async function worktreeDirty(cwd: string): Promise<boolean | null> {
    try {
        const git = Bun.spawn(["git", "status", "--porcelain"], {
            cwd,
            stdout: "pipe",
            stderr: "ignore",
        });
        const output = await new Response(git.stdout).text();
        if ((await git.exited) !== 0) return null;
        return output.trim() !== "";
    } catch {
        return null;
    }
}

function headFromFiles(cwd: string): string | null {
    if (DISCOVERY_ENV.some((name) => process.env[name] !== undefined)) return null;
    try {
        let top = cwd;
        while (!existsSync(join(top, ".git"))) {
            if (dirname(top) === top) return null;
            top = dirname(top);
        }
        let gitDir = join(top, ".git");
        if (statSync(gitDir).isFile()) {
            const link = /^gitdir: (.+)$/m.exec(readFileSync(gitDir, "utf8"));
            if (!link?.[1]) return null;
            gitDir = resolve(top, link[1].trim());
        }
        const commonFile = join(gitDir, "commondir");
        const common = existsSync(commonFile)
            ? resolve(gitDir, readFileSync(commonFile, "utf8").trim())
            : gitDir;
        let head = readFileSync(join(gitDir, "HEAD"), "utf8").trim();
        for (let hops = 0; head.startsWith("ref: ") && hops < 5; hops++) {
            const ref = head.slice("ref: ".length);
            if (!ref.startsWith("refs/heads/")) return null;
            const loose = join(common, ref);
            head = existsSync(loose) ? readFileSync(loose, "utf8").trim() : packedRef(common, ref);
        }
        return OBJECT_ID.test(head) ? head : null;
    } catch {
        return null;
    }
}

/** The object ID `packed-refs` in `common` records for `ref`, or `""` when it records none. */
function packedRef(common: string, ref: string): string {
    const packed = join(common, "packed-refs");
    if (!existsSync(packed)) return "";
    for (const line of readFileSync(packed, "utf8").split("\n")) {
        const [id, name] = line.split(" ");
        if (name === ref && id) return id;
    }
    return "";
}

async function gitRevParse(cwd: string): Promise<string> {
    try {
        const git = Bun.spawn(["git", "rev-parse", "HEAD"], {
            cwd,
            stdout: "pipe",
            stderr: "ignore",
        });
        // The pipe is read once the child has exited and its one line is buffered.
        if ((await git.exited) !== 0) return "unknown";
        return (await new Response(git.stdout).text()).trim();
    } catch {
        return "unknown";
    }
}

if (import.meta.main) {
    try {
        console.log(JSON.stringify(await run(parseArgs(process.argv.slice(2)))));
    } catch (error) {
        console.error((error as Error).message);
        process.exit(2);
    }
}
