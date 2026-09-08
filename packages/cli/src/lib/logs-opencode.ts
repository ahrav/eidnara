import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { sanitizeConfigValue, sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import { type DiagnosticReport, renderDiagnosticsMarkdown } from "./diagnostics-opencode";
import { readFileTail } from "./fs-utils";
import { capBodyToGithubLimit, extractRecentErrors } from "./issue-body";
import { filterLogRecords } from "./log-records";

/**
 *
 *      twice.
 */
export function sanitizeLogContent(content: string): string {
    return sanitizeDiagnosticText(content);
}

function formatTimestamp(date: Date): string {
    const pad = (value: number) => String(value).padStart(2, "0");
    return [
        String(date.getFullYear()),
        pad(date.getMonth() + 1),
        pad(date.getDate()),
        "-",
        pad(date.getHours()),
        pad(date.getMinutes()),
        pad(date.getSeconds()),
    ].join("");
}

export interface BundledIssueReport {
    path: string;
    bodyMarkdown: string;
}

/**
 */
const HISTORIAN_LOG_PATTERNS = [
    /historian failure:/,
    /historian failure recorded:/,
    /historian prompt failed:/,
    /## Historian alert/,
    /historian alert suppressed/,
    /EMERGENCY: aborting session/,
    /historian: prompt attempt \d+ failed:/,
];

function isHistorianLogLine(line: string): boolean {
    return HISTORIAN_LOG_PATTERNS.some((rx) => rx.test(line));
}

/**
 */
function extractHistorianFailureLines(sanitized: string, limit = 30): string[] {
    const matches: string[] = [];
    const lines = sanitized.split(/\r?\n/);
    for (let i = lines.length - 1; i >= 0 && matches.length < limit; i -= 1) {
        if (isHistorianLogLine(lines[i])) {
            matches.push(lines[i]);
        }
    }
    return matches.reverse();
}

/**
 * With a session selected, only records that name that session survive. An
 * untagged record cannot be attributed, and the plugin writes some per-session
 * failures without a tag, so it fails closed rather than into the bundle.
 */
function filterLogLinesBySession(lines: string[], sessionId: string | null): string[] {
    if (!sessionId) return lines;
    // Word boundaries prevent matching `ses_` embedded in longer identifiers.
    const sessionPattern = /\bses_[A-Za-z0-9]{8,32}\b/g;
    return filterLogRecords(lines, (firstLine) => {
        const matches = firstLine.match(sessionPattern);
        if (!matches) return false;
        return matches.every((id) => id === sessionId);
    });
}

const ISSUE_LOG_TAIL_BYTES = 4 * 1024 * 1024;

/**
 * A session filter also narrows the rendered report: other sessions' titles,
 * directories, and historian dumps are as much theirs as their log records.
 */
function narrowReportToSession(report: DiagnosticReport, sessionFilter: string): DiagnosticReport {
    return {
        ...report,
        recentSessions: report.recentSessions.filter(
            (session) => session.sessionId === sessionFilter,
        ),
        historianDumps: {
            ...report.historianDumps,
            byProject: report.historianDumps.byProject.filter((bucket) =>
                bucket.sessionIds.includes(sessionFilter),
            ),
        },
    };
}

/**
 * A log path that exists but cannot be read (permissions, or a directory named
 * by `EIDNARA_LOG_PATH`) yields no lines and an `unreadable` marker, so the
 * rest of the diagnostics still ship.
 */
function readLogTail(logFile: { exists: boolean; path: string }): {
    lines: string[];
    unreadable: string | null;
} {
    if (!logFile.exists) return { lines: [], unreadable: null };
    try {
        return {
            lines: readFileTail(logFile.path, ISSUE_LOG_TAIL_BYTES).split(/\r?\n/),
            unreadable: null,
        };
    } catch (error) {
        return {
            lines: [],
            unreadable: `<log unreadable: ${sanitizeDiagnosticText(error instanceof Error ? error.message : String(error))}>`,
        };
    }
}

export async function bundleIssueReport(
    fullReport: DiagnosticReport,
    description: string,
    title: string,
    sessionFilter: string | null = null,
): Promise<BundledIssueReport> {
    const report =
        sessionFilter === null ? fullReport : narrowReportToSession(fullReport, sessionFilter);
    const LOG_TAIL_LINES = 400;
    const tail = readLogTail(report.logFile);
    const logLines = filterLogLinesBySession(tail.lines, sessionFilter);
    const recentLog =
        tail.unreadable ?? sanitizeLogContent(logLines.slice(-LOG_TAIL_LINES).join("\n")).trim();

    // The 4,000-line window includes historian failures outside the 400-line log tail.
    const historianScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const historianFailureLines = extractHistorianFailureLines(historianScanWindow, 30);

    // The 4,000-line window includes errors outside the 400-line log tail.
    const errorScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const recentErrorLines = extractRecentErrors(errorScanWindow, 20);

    const configBody = JSON.stringify(sanitizeConfigValue(report.eidnaraConfig.flags), null, 2);
    const sanitizedConfigPath = sanitizeDiagnosticText(report.configPaths.eidnaraConfig);
    const sanitizedDescription = sanitizeDiagnosticText(description);
    const sanitizedTitle = sanitizeDiagnosticText(title).trim();

    const rawBodyMarkdown = [
        ...(sanitizedTitle ? ["## Title", sanitizedTitle, ""] : []),
        "## Description",
        sanitizedDescription,
        "",
        "## Environment",
        `- Plugin: v${report.pluginVersion}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        `- OpenCode: ${report.opencodeVersion ?? "not installed"}`,
        "",
        "## Configuration",
        `Config from \`${sanitizedConfigPath}\`:`,
        "```jsonc",
        configBody,
        "```",
        "",
        "## Diagnostics",
        renderDiagnosticsMarkdown(report),
        "",
        "## Historian failure signals (log, sanitized)",
        historianFailureLines.length === 0
            ? "_No historian failure log lines found in recent history._"
            : ["```", historianFailureLines.join("\n"), "```"].join("\n"),
        "",
        "## Recent errors (last 20, sanitized)",
        recentErrorLines.length === 0
            ? "_No error-shaped log lines found in recent history._"
            : ["```", recentErrorLines.join("\n"), "```"].join("\n"),
        "",
        `## Log (last ${LOG_TAIL_LINES} lines, sanitized)`,
        "```",
        recentLog || "<no log output>",
        "```",
    ].join("\n");

    const bodyMarkdown = capBodyToGithubLimit(rawBodyMarkdown);

    const path = join(process.cwd(), `eidnara-issue-${formatTimestamp(new Date())}.md`);
    writeFileSync(path, `${bodyMarkdown}\n`);
    return { path, bodyMarkdown };
}
