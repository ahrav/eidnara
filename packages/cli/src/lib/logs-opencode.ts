import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { sanitizeConfigValue, sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import { type DiagnosticReport, renderDiagnosticsMarkdown } from "./diagnostics-opencode";
import { capBodyToGithubLimit, extractRecentErrors } from "./issue-body";
import { readLogTailLines } from "./log-tail";

/**
 *
 *      twice.
 */
export function sanitizeLogContent(content: string): string {
    return sanitizeDiagnosticText(content);
}

/** A Desktop install reports no version, so absence is decided by the install kind, not the version. */
function describeOpenCodeInstall(report: DiagnosticReport): string {
    if (!report.opencodeInstalled) return "not installed";
    return `${report.opencodeVersion ?? "unknown version"} [${report.opencodeInstallKind}]`;
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
 * Each record starts with a bracketed ISO timestamp. Stack frames following an
 * `Error` record have no session tag, so they inherit that record's filter decision.
 */
const RECORD_START_PATTERN = /^\[\d{4}-\d{2}-\d{2}T/;

function filterLogLinesBySession(lines: string[], sessionId: string | null): string[] {
    if (!sessionId) return lines;
    // Word boundaries prevent matching `ses_` embedded in longer identifiers.
    const otherSessionPattern = /\bses_[A-Za-z0-9]{8,32}\b/g;
    // Lines before the first record start are continuations of a record the tail read cut off,
    // so their session is unknown and they are dropped.
    let keepRecord = false;
    return lines.filter((line) => {
        if (RECORD_START_PATTERN.test(line)) {
            const matches = line.match(otherSessionPattern);
            keepRecord = !matches || matches.every((id) => id === sessionId);
        }
        return keepRecord;
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
        historianDumps: {
            ...report.historianDumps,
            byProject: report.historianDumps.byProject
                .filter((bucket) => bucket.sessionIds.includes(sessionId))
                .map((bucket) => ({
                    ...bucket,
                    primarySessionId: sessionId,
                    sessionIds: [sessionId],
                })),
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

    // The 4,000-line window includes historian failures outside the 400-line log tail.
    const historianScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const historianFailureLines = extractHistorianFailureLines(historianScanWindow, 30);

    // The 4,000-line window includes errors outside the 400-line log tail.
    const errorScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const recentErrorLines = extractRecentErrors(errorScanWindow, 20);

    const userConfigBody = JSON.stringify(sanitizeConfigValue(report.eidnaraConfig.flags), null, 2);
    const projectConfigBody = JSON.stringify(
        sanitizeConfigValue(report.projectConfig.flags),
        null,
        2,
    );
    const sanitizedUserConfigPath = sanitizeDiagnosticText(report.eidnaraConfig.path);
    const sanitizedProjectConfigPath = sanitizeDiagnosticText(report.projectConfig.path);
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
        `- OpenCode: ${describeOpenCodeInstall(report)}`,
        "",
        "## Configuration",
        `User config from \`${sanitizedUserConfigPath}\`${report.eidnaraConfig.exists ? "" : " (missing)"}:`,
        "```jsonc",
        userConfigBody,
        "```",
        `Project config from \`${sanitizedProjectConfigPath}\`${report.projectConfig.exists ? "" : " (missing)"}:`,
        "```jsonc",
        projectConfigBody,
        "```",
        "",
        "## Diagnostics",
        renderDiagnosticsMarkdown(scopedReport),
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
        ...(logReadError ? [`_Log could not be read: ${logReadError}_`] : []),
        "```",
        recentLog || "<no log output>",
        "```",
    ].join("\n");

    const bodyMarkdown = capBodyToGithubLimit(rawBodyMarkdown);

    const path = writeBundleExclusively(
        join(process.cwd(), `eidnara-issue-${formatTimestamp(new Date())}`),
        `${bodyMarkdown}\n`,
    );
    return { path, bodyMarkdown };
}

/**
 * Two bundles created in the same second share a timestamp; exclusive creation plus a
 * numeric suffix keeps the earlier one intact.
 */
function writeBundleExclusively(basePath: string, contents: string): string {
    for (let attempt = 0; ; attempt += 1) {
        const path = attempt === 0 ? `${basePath}.md` : `${basePath}-${attempt + 1}.md`;
        try {
            writeFileSync(path, contents, { flag: "wx" });
            return path;
        } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        }
    }
}
