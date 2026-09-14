import { join } from "node:path";
import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import {
    type DiagnosticReport,
    describeProbeText,
    renderDiagnosticsMarkdown,
} from "./diagnostics-opencode";
import { writeNewFile } from "./fs-utils";
import { scopeDumpBucketsToSession } from "./history_summarizer-dumps";
import { capBodyToGithubLimit, codeFenceFor, extractRecentErrors } from "./issue-body";
import { filterLogRecords } from "./log-records";
import { readLogTailLines } from "./log-tail";

/**
 *
 *      twice.
 */
export function sanitizeLogContent(content: string): string {
    return sanitizeDiagnosticText(content);
}

/**
 * A Desktop install reports no version, so absence is decided by the install kind, not the version.
 * The version text is external process output and is sanitized like any other probe result.
 */
function describeOpenCodeInstall(report: DiagnosticReport): string {
    if (!report.opencodeInstalled) return "not installed";
    const version = report.opencodeVersion ? describeProbeText(report.opencodeVersion) : null;
    return `${version ?? "unknown version"} [${report.opencodeInstallKind}]`;
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
const HISTORY_SUMMARIZER_LOG_PATTERNS = [
    /history_summarizer failure:/,
    /history_summarizer failure recorded:/,
    /history_summarizer prompt failed:/,
    /## HistorySummarizer alert/,
    /history_summarizer alert suppressed/,
    /EMERGENCY: aborting session/,
    /history_summarizer: prompt attempt \d+ failed:/,
];

function isHistorySummarizerLogLine(line: string): boolean {
    return HISTORY_SUMMARIZER_LOG_PATTERNS.some((rx) => rx.test(line));
}

/**
 */
function extractHistorySummarizerFailureLines(sanitized: string, limit = 30): string[] {
    const matches: string[] = [];
    const lines = sanitized.split(/\r?\n/);
    for (let i = lines.length - 1; i >= 0 && matches.length < limit; i -= 1) {
        if (isHistorySummarizerLogLine(lines[i])) {
            matches.push(lines[i]);
        }
    }
    return matches.reverse();
}

/**
 * With a session selected, only records whose first line names that session
 * survive. An untagged record cannot be attributed, and the plugin writes some
 * per-session failures without a tag, so it fails closed rather than into the
 * bundle. Stack frames following an `Error` record have no session tag and
 * inherit that record's decision.
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

function scopeReportToSession(
    report: DiagnosticReport,
    sessionId: string | null,
): DiagnosticReport {
    if (!sessionId) return report;
    return {
        ...report,
        recentSessions: report.recentSessions.filter((session) => session.sessionId === sessionId),
        history_summarizerDumps: {
            ...report.history_summarizerDumps,
            byProject: scopeDumpBucketsToSession(
                report.history_summarizerDumps.byProject,
                sessionId,
            ),
        },
    };
}

export async function bundleIssueReport(
    report: DiagnosticReport,
    description: string,
    title: string,
    sessionFilter: string | null = null,
): Promise<BundledIssueReport> {
    const LOG_TAIL_LINES = 400;
    const scopedReport = scopeReportToSession(report, sessionFilter);
    // A log statted during diagnostics can become unreadable before bundling.
    let allLogLines: string[] = [];
    let logReadError: string | null = null;
    if (report.logFile.exists) {
        try {
            allLogLines = readLogTailLines(report.logFile.path);
        } catch (error) {
            logReadError = sanitizeDiagnosticText(
                error instanceof Error ? error.message : String(error),
            );
        }
    }
    const logLines = filterLogLinesBySession(allLogLines, sessionFilter);
    const recentLog = sanitizeLogContent(logLines.slice(-LOG_TAIL_LINES).join("\n")).trim();

    // The 4,000-line window includes history_summarizer failures outside the 400-line log tail.
    const history_summarizerScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const history_summarizerFailureLines = extractHistorySummarizerFailureLines(
        history_summarizerScanWindow,
        30,
    );

    // The 4,000-line window includes errors outside the 400-line log tail.
    const errorScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const recentErrorLines = extractRecentErrors(errorScanWindow, 20);

    const sanitizedUserConfigPath = sanitizeDiagnosticText(report.eidnaraConfig.path);
    const sanitizedProjectConfigPath = sanitizeDiagnosticText(report.projectConfig.path);
    const sanitizedDescription = sanitizeDiagnosticText(description);
    const sanitizedTitle = sanitizeDiagnosticText(title).trim();
    const history_summarizerBlock = history_summarizerFailureLines.join("\n");
    const errorBlock = recentErrorLines.join("\n");
    const fence = codeFenceFor(history_summarizerBlock, errorBlock, recentLog);

    const rawBodyMarkdown = [
        ...(sanitizedTitle ? ["## Title", sanitizedTitle, ""] : []),
        "## Description",
        sanitizedDescription,
        "",
        "## Environment",
        `- Plugin: v${report.pluginVersion}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        `- OpenCode: ${describeOpenCodeInstall(report)}`,
        "",
        "## Configuration",
        `User config from \`${sanitizedUserConfigPath}\`${report.eidnaraConfig.exists ? "" : " (missing)"}`,
        `Project config from \`${sanitizedProjectConfigPath}\`${report.projectConfig.exists ? "" : " (missing)"}`,
        "Sanitized flags for both tiers are listed under Diagnostics.",
        "",
        "## Diagnostics",
        renderDiagnosticsMarkdown(scopedReport),
        "",
        "## HistorySummarizer failure signals (log, sanitized)",
        history_summarizerFailureLines.length === 0
            ? "_No history_summarizer failure log lines found in recent history._"
            : [fence, history_summarizerBlock, fence].join("\n"),
        "",
        "## Recent errors (last 20, sanitized)",
        recentErrorLines.length === 0
            ? "_No error-shaped log lines found in recent history._"
            : [fence, errorBlock, fence].join("\n"),
        "",
        `## Log (last ${LOG_TAIL_LINES} lines, sanitized)`,
        ...(logReadError ? [`_Log could not be read: ${logReadError}_`] : []),
        fence,
        recentLog || "<no log output>",
        fence,
    ].join("\n");

    const bodyMarkdown = capBodyToGithubLimit(rawBodyMarkdown);

    const path = writeNewFile(
        join(process.cwd(), `eidnara-issue-${formatTimestamp(new Date())}`),
        `${bodyMarkdown}\n`,
    );
    return { path, bodyMarkdown };
}
