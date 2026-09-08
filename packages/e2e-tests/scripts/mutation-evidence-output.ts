/**
 * Stack frames, code context, and build output carry absolute paths and raw process output, which `incidents/README.md` ("Publication and privacy") keeps out of committed artifacts.
 */

const CARGO_EVIDENCE_LINE =
    /^(test |test result:|thread '|assertion `|\s+left:|\s+right:|failures:)/;

/**
 * Bun prints `(pass)`/`(fail)` verdicts, `error: expect(...)` assertions with their diff, a custom assertion message as `error: <message>`, and the summary counts.
 * A custom message is kept only when a drill declares it in `ASSERTION_MESSAGES`; every other `error:` line is exception text and is dropped.
 */
const ASSERTION_MESSAGES = ["queued ctx_reduce drop must be pending before the bust"] as const;

function escapeRegExp(text: string): string {
    return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

const BUN_EVIDENCE_LINE = new RegExp(
    [
        "^\\s*(?:",
        [
            "\\(pass\\)",
            "\\(fail\\)",
            "\\(skip\\)",
            "error: expect\\(",
            `error: (?:${ASSERTION_MESSAGES.map(escapeRegExp).join("|")})$`,
            "[-+] (?:Expected|Received)",
            "Expected:",
            "Received:",
            "\\d+ pass$",
            "\\d+ fail$",
            "\\d+ skip$",
            "\\d+ expect\\(\\) calls$",
            "Ran \\d+ tests?",
        ].join("|"),
        ")",
    ].join(""),
);

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
