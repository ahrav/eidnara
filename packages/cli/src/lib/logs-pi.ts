import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import {
    type PiDiagnosticReport,
    renderDiagnosticsMarkdown,
    sanitizeString,
} from "./diagnostics-pi";
import { capBodyToGithubLimit, extractRecentErrors } from "./issue-body";

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

/** `[eidnara][<sessionId>]` identifies session-scoped log lines. */
const SESSION_TAG_PATTERN = /\[eidnara\]\[([^\]]+)\]/;

/**
 * Pi accepts UUIDs and caller-chosen session ids, so a UUID tag belongs to
 * another session unless it equals the selected id, and a non-UUID tag does so
 * only when the report lists it. Non-session tags such as `[eidnara][pi-status]`
 * pass through.
 */
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function filterLogLinesBySession(
    lines: string[],
    sessionId: string | null,
    knownSessionIds: readonly string[],
): string[] {
    if (!sessionId) return lines;
    const otherSessionIds = new Set(knownSessionIds.filter((id) => id !== sessionId));
    return lines.filter((line) => {
        const tagged = SESSION_TAG_PATTERN.exec(line)?.[1];
        if (tagged === undefined || tagged === sessionId) return true;
        return !otherSessionIds.has(tagged) && !UUID_PATTERN.test(tagged);
    });
}

export async function bundleIssueReport(
    report: PiDiagnosticReport,
    description: string,
    title: string,
    options: { cwd?: string; now?: Date; sessionFilter?: string | null } = {},
): Promise<BundledIssueReport> {
    const LOG_TAIL_LINES = 400;
    const allLogLines = report.logFile.exists
        ? readFileSync(report.logFile.path, "utf-8").split(/\r?\n/)
        : [];
    const logLines = filterLogLinesBySession(
        allLogLines,
        options.sessionFilter ?? null,
        report.recentSessions.map((session) => session.sessionId),
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
        `- Pi: ${report.piVersion ?? "not installed"}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        "",
        "## Diagnostics",
        renderDiagnosticsMarkdown(report),
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

    const cwd = options.cwd ?? process.cwd();
    const path = join(cwd, `eidnara-pi-issue-${formatTimestamp(options.now ?? new Date())}.md`);
    writeFileSync(path, `${bodyMarkdown}\n`);
    return { path, bodyMarkdown };
}
