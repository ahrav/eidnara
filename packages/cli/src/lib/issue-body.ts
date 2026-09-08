/**
 *
 *
 *      truncation.
 *
 * Body truncation removes oldest main-log lines before other report sections when a report exceeds its body budget.
 *
 */

/**
 * `MAX_GITHUB_BODY_BYTES` reserves 5,536 bytes below GitHub's 65,536-byte body limit.
 */
export const MAX_GITHUB_BODY_BYTES = 60_000;

const LOG_TRUNCATION_MARKER = "[truncated for GitHub 64KB limit — older log lines dropped]\n";
const FINAL_TRUNCATION_MARKER = "\n\n[truncated further to fit GitHub body limit]\n";
const FALLBACK_TRUNCATION_MARKER = "\n\n[truncated for GitHub 64KB limit]\n";
const FENCE = "```";
const FENCE_CLOSE = `\n${FENCE}`;
const LOG_HEADING = "## Log (last";
const OTHER_SECTION_HEADING = /^## (?!Log \(last)/m;

/**
 * The stack-frame patterns retain frames to identify the failing call site.
 *
 * Count telemetry such as `0 failed` or `4 failed;` is excluded when an integer precedes `failed` and line end, `,`, `;`, or `)` follows it.
 */
const ERROR_LOG_PATTERNS = [
    /\bfailed\b(?!\s*(?:$|[,;)]))|(?<!(?:^|[\s(;,:])\d+\s+)\bfailed\b/i,
    /\b\w*error:\s/i,
    /\bEMERGENCY\b/,
    /\bexception\b/i,
    /^\s+at\s+(?:async\s+|new\s+)?[\w.<>$]+(?:\s+\[as\s+[\w$]+\])?\s+\(/,
    /^\s+at\s+(?:async\s+)?(?:file:|node_modules\/|[^/\s]+:\d+)/,
];

function isErrorLogLine(line: string): boolean {
    return ERROR_LOG_PATTERNS.some((rx) => rx.test(line));
}

/**
 * `extractRecentErrors` returns matches oldest-first so issue bodies read chronologically.
 *
 */
export function extractRecentErrors(sanitized: string, limit = 20): string[] {
    const matches: string[] = [];
    const lines = sanitized.split(/\r?\n/);
    for (let i = lines.length - 1; i >= 0 && matches.length < limit; i -= 1) {
        if (isErrorLogLine(lines[i])) {
            matches.push(lines[i]);
        }
    }
    return matches.reverse();
}

/**
 * A fence one backtick longer than any run inside the fenced content cannot be closed early
 * by that content (CommonMark closes a fence only with a run at least as long as the opener).
 */
export function codeFenceFor(...blocks: string[]): string {
    let longest = 0;
    for (const block of blocks) {
        for (const run of block.match(/`+/g) ?? []) {
            if (run.length > longest) longest = run.length;
        }
    }
    return "`".repeat(Math.max(3, longest + 1));
}

/**
 * When the expected log fence exists, the function drops oldest log lines before enforcing the final limit.
 * The main log starts at the last `## Log (last` heading that opens the final fenced block before the closing fence and leaves a positive log budget.
 *
 * `capBodyToGithubLimit` measures its budget in UTF-8 bytes.
 */
export function capBodyToGithubLimit(
    body: string,
    maxBytes: number = MAX_GITHUB_BODY_BYTES,
): string {
    if (Buffer.byteLength(body, "utf8") <= maxBytes) return body;

    if (body.lastIndexOf(LOG_HEADING) === -1) {
        return truncateWithBalancedFences(body, maxBytes, FALLBACK_TRUNCATION_MARKER);
    }

    let capped = body;

    const fenceCloseIdx = body.lastIndexOf(FENCE_CLOSE);
    const logStart = findMainLogStart(body, fenceCloseIdx, maxBytes);
    if (logStart === -1) return enforceFinalBodyLimit(body, maxBytes);

    const head = body.slice(0, logStart);
    const log = body.slice(logStart, fenceCloseIdx);
    const tail = body.slice(fenceCloseIdx);

    const logBudget = logBudgetFor(body, logStart, fenceCloseIdx, maxBytes);
    if (logBudget <= 0) {
        capped = `${head}${LOG_TRUNCATION_MARKER}${tail}`;
        return enforceFinalBodyLimit(capped, maxBytes);
    }

    // A trailing blank line would otherwise remain as the sole kept element,
    // dropping an oversized newest entry instead of truncating it.
    const lines = log.replace(/\n+$/, "").split("\n");
    let keepLines = lines;
    let kept = keepLines.join("\n");
    while (Buffer.byteLength(kept, "utf8") > logBudget && keepLines.length > 1) {
        // Dropping batches of oldest lines accelerates convergence for oversized logs.
        const dropCount = Math.max(1, Math.floor(keepLines.length * 0.05));
        keepLines = keepLines.slice(dropCount);
        kept = keepLines.join("\n");
    }
    // A single log line can exceed `logBudget` after all other lines are removed.
    if (Buffer.byteLength(kept, "utf8") > logBudget) {
        kept = truncateToByteBudget(kept, logBudget);
    }

    capped = `${head}${LOG_TRUNCATION_MARKER}${kept}${tail}`;
    return enforceFinalBodyLimit(capped, maxBytes);
}

/**
 * Search backward so a duplicate description heading cannot outrank the main log.
 * Ignore headings whose log begins after `fenceCloseIdx` because they occur inside the main log.
 * Skip candidates whose fixed bytes leave no space for log content.
 */
function findMainLogStart(body: string, fenceCloseIdx: number, maxBytes: number): number {
    let latest = -1;
    let idx = body.lastIndexOf(LOG_HEADING);
    while (idx !== -1) {
        const fenceOpenIdx = body.indexOf(FENCE_CLOSE, idx);
        if (fenceOpenIdx !== -1) {
            // The opener may be longer than three backticks (`codeFenceFor`); the log starts after its line.
            const openerLineEnd = body.indexOf("\n", fenceOpenIdx + 1);
            const logStart = openerLineEnd === -1 ? body.length : openerLineEnd + 1;
            if (logStart <= fenceCloseIdx) {
                if (latest === -1) {
                    latest = logStart;
                } else if (OTHER_SECTION_HEADING.test(body.slice(logStart, latest))) {
                    break;
                }
                if (logBudgetFor(body, logStart, fenceCloseIdx, maxBytes) > 0) return logStart;
            }
        }
        idx = idx === 0 ? -1 : body.lastIndexOf(LOG_HEADING, idx - 1);
    }
    return latest;
}

function logBudgetFor(
    body: string,
    logStart: number,
    fenceCloseIdx: number,
    maxBytes: number,
): number {
    const overheadBytes =
        Buffer.byteLength(body.slice(0, logStart), "utf8") +
        Buffer.byteLength(body.slice(fenceCloseIdx), "utf8");
    return maxBytes - overheadBytes - Buffer.byteLength(LOG_TRUNCATION_MARKER, "utf8");
}

function enforceFinalBodyLimit(body: string, maxBytes: number): string {
    if (Buffer.byteLength(body, "utf8") <= maxBytes) return body;
    return truncateWithBalancedFences(body, maxBytes, FINAL_TRUNCATION_MARKER);
}

/**
 * A byte cut inside a fence line can leave an unclosed Markdown fence.
 * Drop a trailing partial backtick line and close any open fence.
 */
function truncateWithBalancedFences(body: string, maxBytes: number, marker: string): string {
    const markerBytes = Buffer.byteLength(marker, "utf8");
    const fenceBytes = Buffer.byteLength(FENCE_CLOSE, "utf8");
    if (markerBytes + fenceBytes >= maxBytes) {
        return truncateToByteBudget(marker, maxBytes);
    }
    let kept = truncateToByteBudget(body, maxBytes - markerBytes - fenceBytes);
    const lastLineStart = kept.lastIndexOf("\n") + 1;
    if (kept.startsWith("`", lastLineStart)) {
        kept = kept.slice(0, Math.max(0, lastLineStart - 1));
    }
    if (hasOpenFence(kept)) kept += FENCE_CLOSE;
    return kept + marker;
}

function hasOpenFence(markdown: string): boolean {
    let open = false;
    for (const line of markdown.split("\n")) {
        if (line.startsWith(FENCE)) open = !open;
    }
    return open;
}

/**
 * Naive `Buffer.subarray(...).toString("utf8")` can split a multibyte code point.
 * A partial UTF-8 code point decodes as U+FFFD (3 bytes).
 * Replacement with U+FFFD can make the decoded output exceed `maxBytes`.
 * happens mid-character.
 *
 */
function truncateToByteBudget(input: string, maxBytes: number): string {
    if (maxBytes <= 0) return "";
    const buf = Buffer.from(input, "utf8");
    if (buf.length <= maxBytes) return input;
    let end = maxBytes;
    while (end > 0 && (buf[end] & 0b1100_0000) === 0b1000_0000) {
        end -= 1;
    }
    return buf.subarray(0, end).toString("utf8");
}
