import { writeFileSync } from "node:fs";
import { join } from "node:path";

import {
    type PiDiagnosticReport,
    renderDiagnosticsMarkdown,
    sanitizeString,
} from "./diagnostics-pi";
import { readFileTail } from "./fs-utils";
import { capBodyToGithubLimit, extractRecentErrors } from "./issue-body";
import { filterLogRecords } from "./log-records";

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
 * The filter retains these tags and drops every other tag and every untagged
 * record, so it never has to infer a session id's format or attribute an
 * unclassified record. `[eidnara][pi]` and `[eidnara][pi-status]` are not
 * listed: the plugin writes per-session command output under them.
 */
const NON_SESSION_TAGS: ReadonlySet<string> = new Set(["global"]);

function filterLogLinesBySession(lines: string[], sessionId: string | null): string[] {
    if (!sessionId) return lines;
    return filterLogRecords(lines, (firstLine) => {
        const tagged = SESSION_TAG_PATTERN.exec(firstLine)?.[1];
        return tagged !== undefined && (tagged === sessionId || NON_SESSION_TAGS.has(tagged));
    });
}

const ISSUE_LOG_TAIL_BYTES = 4 * 1024 * 1024;

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
            unreadable: `<log unreadable: ${sanitizeString(error instanceof Error ? error.message : String(error))}>`,
        };
    }
}

export async function bundleIssueReport(
    report: PiDiagnosticReport,
    description: string,
    title: string,
    options: { cwd?: string; now?: Date; sessionFilter?: string | null } = {},
): Promise<BundledIssueReport> {
    const LOG_TAIL_LINES = 400;
    const tail = readLogTail(report.logFile);
    const logLines = filterLogLinesBySession(tail.lines, options.sessionFilter ?? null);
    const recentLog =
        tail.unreadable ?? sanitizeLogContent(logLines.slice(-LOG_TAIL_LINES).join("\n")).trim();

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
