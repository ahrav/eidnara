#!/usr/bin/env bun
/**
 * `eval:compression-fidelity`: assembles one baseline and one candidate evidence directory into
 * a private manifest and per-scenario report. Usage:
 *
 *   bun run eval:compression-fidelity --baseline <dir> --candidate <dir>
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
import { assembleEvidence, loadArm, REPORT_SCHEMA } from "../src/compression-fidelity/evidence";

export const USAGE =
    "eval:compression-fidelity --baseline <dir> --candidate <dir> --out <dir> [--corpus <path>] [--mode offline|live]";

export interface EvalArgs {
    baseline: string;
    candidate: string;
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
        if (!["baseline", "candidate", "out", "corpus", "mode"].includes(name)) {
            throw new Error(`unknown flag --${name}: ${USAGE}`);
        }
    }
    return {
        baseline: required("baseline"),
        candidate: required("candidate"),
        out: required("out"),
        corpus: values.get("corpus") ?? COMPRESSION_FIDELITY_CORPUS_PATH,
        mode,
    };
}

/**
 * Runs the assembly and returns the written paths and whether the comparison was accepted. The
 * report carries each scenario's execution and deterministic columns and accepts no arm, since
 * it assembles no review, control, or cost evidence.
 */
export function run(args: EvalArgs): { manifest: string; report: string; accepted: boolean } {
    const corpus = readCompressionFidelityCorpus(args.corpus);
    const sha = COMPRESSION_FIDELITY_CORPUS_SHA256;
    const git = spawnSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" });
    const assembled = assembleEvidence({
        corpus,
        corpusPath: args.corpus,
        corpusSha256: sha,
        revision: git.status === 0 ? git.stdout.trim() : "unknown",
        mode: args.mode,
        baseline: loadArm(args.baseline, corpus, sha),
        candidate: loadArm(args.candidate, corpus, sha),
    });
    const report = {
        schema: REPORT_SCHEMA,
        corpus_sha256: sha,
        arms: assembled.arms.map((side) => ({
            label: side.label,
            identity_errors: side.identity_errors,
            reached_scenarios: side.reached_scenarios,
            missing_scenarios: side.missing_scenarios,
            rows: side.rows.map((row) => ({
                scenario: row.scenario.id,
                case: row.case,
                execution: row.execution,
                deterministic: row.deterministic,
            })),
            accepted: false,
            withheld: [
                ...side.identity_errors,
                "no review, control, or cost evidence is assembled",
            ],
        })),
        comparison: { refused: assembled.refused, treatment: assembled.treatment },
        accepted: false,
    };
    return {
        manifest: publishPrivateJson(assembled.manifest, args.out, "manifest.json"),
        report: publishPrivateJson(report, args.out, "report.json"),
        accepted: false,
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
