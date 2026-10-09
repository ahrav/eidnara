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

import { spawnSync } from "node:child_process";
import { publishPrivateJson } from "../src/atomic-publish";
import {
    COMPRESSION_FIDELITY_CORPUS_PATH,
    COMPRESSION_FIDELITY_CORPUS_SHA256,
    readCompressionFidelityCorpus,
} from "../src/compression-fidelity/corpus";
import { loadArm } from "../src/compression-fidelity/evidence";
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
export function run(args: EvalArgs): { manifest: string; report: string; accepted: boolean } {
    const corpus = readCompressionFidelityCorpus(args.corpus);
    const sha = COMPRESSION_FIDELITY_CORPUS_SHA256;
    const git = spawnSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" });
    const { manifest, report } = evaluate({
        corpus,
        corpusPath: args.corpus,
        corpusSha256: sha,
        revision: git.status === 0 ? git.stdout.trim() : "unknown",
        mode: args.mode,
        baseline: loadArm(args.baseline, corpus, sha),
        candidate: loadArm(args.candidate, corpus, sha),
        reviews: loadReviews(args.reviews, sha),
    });
    return {
        manifest: publishPrivateJson(manifest, args.out, "manifest.json"),
        report: publishPrivateJson(report, args.out, "report.json"),
        accepted: report.accepted === true,
    };
}

if (import.meta.main) {
    try {
        console.log(JSON.stringify(run(parseArgs(process.argv.slice(2)))));
    } catch (error) {
        console.error((error as Error).message);
        process.exit(2);
    }
}
