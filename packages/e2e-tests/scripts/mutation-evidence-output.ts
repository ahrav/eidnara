/**
 * Stack frames, code context, and build output carry absolute paths and raw process output, which `incidents/README.md` ("Publication and privacy") keeps out of committed artifacts.
 */

const CARGO_EVIDENCE_LINE =
    /^(test |test result:|thread '|assertion `|\s+left:|\s+right:|failures:)/;

/** Bun prints `(pass)`/`(fail)` verdicts, `error: expect(...)` assertions with their diff, and the summary counts. */
const BUN_EVIDENCE_LINE =
    /^\s*(\(pass\)|\(fail\)|\(skip\)|error: |[-+] (Expected|Received)|Expected:|Received:|\d+ pass$|\d+ fail$|\d+ skip$|\d+ expect\(\) calls$|Ran \d+ tests?)/;

function keepLines(output: string, pattern: RegExp): string {
    return output
        .split("\n")
        .filter((line) => pattern.test(line))
        .join("\n");
}

export function cargoTestEvidence(output: string): string {
    return keepLines(output, CARGO_EVIDENCE_LINE);
}

export function bunTestEvidence(output: string): string {
    return keepLines(output, BUN_EVIDENCE_LINE);
}
