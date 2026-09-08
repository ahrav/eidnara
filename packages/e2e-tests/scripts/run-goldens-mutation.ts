#!/usr/bin/env bun

import { readFileSync, writeFileSync } from "node:fs";
import { relative, resolve } from "node:path";

type CommandResult = {
    exit_status: number;
    output: string;
};

type GoldenCase = {
    input: { messages: Array<{ content: Array<{ kind: { text: string } }> }> };
};

type GoldenFile = { cases: GoldenCase[] };

const e2eRoot = resolve(import.meta.dir, "..");
const repoRoot = resolve(e2eRoot, "../..");
const goldenPath = resolve(repoRoot, "crates/daemon/testdata/differential-golden.json");
const command =
    "cargo test -p daemon --lib dg_goldens_match_ts_wire_surface_and_gate_labels --locked";

/** Each DG family perturbs the first text block of its own case so the rendered wire drifts from `expected.wire`. */
const families: Record<string, { caseIndex: number; family: string }> = {
    "1": { caseIndex: 0, family: "postprocess-gates" },
    "2": { caseIndex: 1, family: "marker-representation" },
    "3": { caseIndex: 2, family: "escalation-bands" },
};

const TEXT_PATH = (index: number) => `cases[${index}].input.messages[0].content[0].kind.text`;

/* Compile noise and target paths vary per host; the harness lines are the evidence. */
const EVIDENCE_LINE = /^(test |test result:|thread '|assertion `|\s+left:|\s+right:|failures:)/;

function runGoldens(): CommandResult {
    const result = Bun.spawnSync({
        cmd: command.split(" "),
        cwd: repoRoot,
        stdout: "pipe",
        stderr: "pipe",
        env: process.env,
    });
    const decoder = new TextDecoder();
    const output = `${decoder.decode(result.stdout)}${decoder.decode(result.stderr)}`
        .split("\n")
        .filter((line) => EVIDENCE_LINE.test(line))
        .join("\n");
    return { exit_status: result.exitCode, output };
}

function mutatedGolden(
    text: string,
    caseIndex: number,
): { before: string; after: string; mutated: string } {
    const golden = JSON.parse(text) as GoldenFile;
    const block = golden.cases[caseIndex]?.input.messages[0]?.content[0]?.kind;
    if (!block || typeof block.text !== "string") {
        throw new Error(`DG-${caseIndex + 1}: ${TEXT_PATH(caseIndex)} is not a text block`);
    }
    const before = block.text;
    const after = `${before}x`;
    block.text = after;
    return { before, after, mutated: `${JSON.stringify(golden, null, 2)}\n` };
}

const drill = Bun.argv[2];
const target = drill ? families[drill] : undefined;
if (!drill || !target) {
    console.error(`usage: bun scripts/run-goldens-mutation.ts ${Object.keys(families).join("|")}`);
    process.exit(2);
}

const original = readFileSync(goldenPath, "utf8");
const { before, after, mutated } = mutatedGolden(original, target.caseIndex);
const name = `DG_${drill}_ONE_BYTE_INPUT`;
writeFileSync(goldenPath, mutated);
let observedFailure: CommandResult;
try {
    observedFailure = runGoldens();
} finally {
    writeFileSync(goldenPath, original);
}
const revertedRerun = runGoldens();

if (observedFailure.exit_status === 0) {
    throw new Error(`${name}: mutation did not redden the goldens test`);
}
if (revertedRerun.exit_status !== 0) {
    throw new Error(`${name}: reverted goldens test did not pass`);
}

const record = {
    golden_family: target.family,
    command,
    mutations: [
        {
            name,
            applied_diff: {
                path: relative(repoRoot, goldenPath),
                before: `${TEXT_PATH(target.caseIndex)}='${before}'`,
                after: `${TEXT_PATH(target.caseIndex)}='${after}'`,
                changed: mutated !== original,
            },
            observed_failure: observedFailure,
            reverted_rerun: { ...revertedRerun, status: "pass" },
            adequacy_finding: null,
        },
    ],
};
const recordPath = resolve(e2eRoot, `mutations/goldens-dg-${drill}.json`);
writeFileSync(recordPath, `${JSON.stringify(record, null, 2)}\n`);
console.log(`wrote ${recordPath}`);
