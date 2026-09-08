import { join } from "node:path";

import {
    type PiDiagnosticReport,
    renderDiagnosticsMarkdown,
    sanitizeString,
} from "./diagnostics-pi";
import { writeNewFile } from "./fs-utils";
import { scopeDumpBucketsToSession } from "./historian-dumps";
import { capBodyToGithubLimit, extractRecentErrors } from "./issue-body";
import { filterLogRecords } from "./log-records";
import { readLogTailLines } from "./log-tail";

export { readLogTailLines } from "./log-tail";

export function sanitizeLogContent(content: string): string {
    return sanitizeString(content);
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

const TAG_PATTERN = /\[eidnara\]\[([^\]]+)\]/g;

/**
 * Tags that belong to no session and survive every session filter.
 * `[eidnara][pi]` and `[eidnara][pi-status]` are not listed: the plugin writes
 * per-session command output under them, so they fail closed with the
 * session-tagged records.
 */
const NON_SESSION_TAGS: ReadonlySet<string> = new Set(["global"]);

/**
 * A record survives only when it carries tags and every tag is the picked
 * session or a non-session tag. An untagged record cannot be attributed, and
 * the plugin writes some per-session failures without a tag, so it fails
 * closed. `sessionId` may end with `_<uuid>` because log tags carry the bare
 * id while a session file name carries a timestamp prefix. Continuation lines
 * keep the preceding record's decision.
 */
function filterLogLinesBySession(lines: string[], sessionId: string | null): string[] {
    if (!sessionId) return lines;
    const isWanted = (tag: string) =>
        tag === sessionId || sessionId.endsWith(`_${tag}`) || NON_SESSION_TAGS.has(tag);
    return filterLogRecords(lines, (firstLine) => {
        const tags = [...firstLine.matchAll(TAG_PATTERN)].map((match) => match[1] ?? "");
        return tags.length > 0 && tags.every(isWanted);
    });
}

/**
 * A log line starting with up to three spaces and three or more backticks
 * would close the bundle fence; escaping its first backtick prevents that.
 * A bare carriage return also starts a Markdown line, so a fence after one is escaped too.
 */
function escapeFenceOpeners(lines: string[]): string[] {
    return lines.map((line) => line.replace(/(^|\r)( {0,3})(`{3,})/g, "$1$2\\$3"));
}

/**
 * With a session selected, the configuration and log sections describe that session only, so
 * the session list and the per-project dump metadata are narrowed to match.
 */
function scopeReportToSession(
    report: PiDiagnosticReport,
    sessionId: string | null,
): PiDiagnosticReport {
    if (!sessionId) return report;
    return {
        ...report,
        recentSessions: report.recentSessions.filter((session) => session.sessionId === sessionId),
        historianDumps: {
            ...report.historianDumps,
            byProject: scopeDumpBucketsToSession(report.historianDumps.byProject, sessionId),
        },
    };
}

export async function bundleIssueReport(
    report: PiDiagnosticReport,
    description: string,
    title: string,
    options: { cwd?: string; now?: Date; sessionFilter?: string | null } = {},
): Promise<BundledIssueReport> {
    const LOG_TAIL_LINES = 400;
    let allLogLines: string[] = [];
    let logReadError: string | null = null;
    if (report.logFile.exists) {
        try {
            allLogLines = readLogTailLines(report.logFile.path);
        } catch (error) {
            logReadError = error instanceof Error ? error.message : String(error);
        }
    }
    const logLines = escapeFenceOpeners(
        filterLogLinesBySession(allLogLines, options.sessionFilter ?? null),
    );
    const recentLog = sanitizeLogContent(logLines.slice(-LOG_TAIL_LINES).join("\n")).trim();

    // The error scan uses 4,000 lines so trailing log output does not exclude earlier errors.
    const errorScanWindow = sanitizeLogContent(logLines.slice(-4000).join("\n"));
    const recentErrorLines = extractRecentErrors(errorScanWindow, 20);

    const rawBodyMarkdown = [
        "## Title",
        `[pi] ${sanitizeString(title)}`,
        "",
        "## Description",
        sanitizeString(description),
        "",
        "## Environment",
        `- Pi plugin: v${report.pluginVersion}`,
        `- Pi: ${report.piInstalled ? (report.piVersion ?? "installed, version unavailable") : "not installed"}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        "",
        "## Diagnostics",
        renderDiagnosticsMarkdown(scopeReportToSession(report, options.sessionFilter ?? null)),
        "",
        "## Recent errors (last 20, sanitized)",
        recentErrorLines.length === 0
            ? "_No error-shaped log lines found in recent history._"
            : ["```", recentErrorLines.join("\n"), "```"].join("\n"),
        "",
        `## Log (last ${LOG_TAIL_LINES} lines, sanitized)`,
        "```",
        logReadError
            ? `<log unreadable: ${sanitizeLogContent(logReadError)}>`
            : recentLog || "<no log output>",
        "```",
    ].join("\n");

    const bodyMarkdown = capBodyToGithubLimit(rawBodyMarkdown);

    const cwd = options.cwd ?? process.cwd();
    const stem = join(cwd, `eidnara-pi-issue-${formatTimestamp(options.now ?? new Date())}`);
    const path = writeNewFile(stem, `${bodyMarkdown}\n`);
    return { path, bodyMarkdown };
}
