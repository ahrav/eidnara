import { closeSync, fstatSync, openSync, readSync, writeFileSync } from "node:fs";
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

/** The logger appends without rotation, so the bundle reads a bounded tail. */
const LOG_TAIL_MAX_BYTES = 8 * 1024 * 1024;

export function readLogTailLines(path: string, maxBytes = LOG_TAIL_MAX_BYTES): string[] {
    const fd = openSync(path, "r");
    try {
        const size = fstatSync(fd).size;
        const start = Math.max(0, size - maxBytes);
        const buffer = Buffer.alloc(size - start);
        const bytesRead = readSync(fd, buffer, 0, buffer.length, start);
        const lines = buffer.toString("utf-8", 0, bytesRead).split(/\r?\n/);
        // A mid-file start lands inside a line, so the first entry is a fragment.
        if (start > 0) lines.shift();
        return lines;
    } finally {
        closeSync(fd);
    }
}

/** `sessionLog` writes `[eidnara][<uuid>]`; a line with no UUID tag belongs to no single session. */
const SESSION_TAG_PATTERN =
    /\[eidnara\]\[([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\]/gi;

/** `sessionId` is a Pi JSONL stem `<timestamp>_<uuid>` while log tags carry the bare UUID. */
function filterLogLinesBySession(lines: string[], sessionId: string | null): string[] {
    if (!sessionId) return lines;
    const isWanted = (tag: string) => tag === sessionId || sessionId.endsWith(`_${tag}`);
    return lines.filter((line) => {
        const tags = [...line.matchAll(SESSION_TAG_PATTERN)].map((match) => match[1] ?? "");
        if (tags.length === 0) return true;
        return tags.every(isWanted);
    });
}

export async function bundleIssueReport(
    report: PiDiagnosticReport,
    description: string,
    title: string,
    options: { cwd?: string; now?: Date; sessionFilter?: string | null } = {},
): Promise<BundledIssueReport> {
    const LOG_TAIL_LINES = 400;
    const allLogLines = report.logFile.exists ? readLogTailLines(report.logFile.path) : [];
    const logLines = filterLogLinesBySession(allLogLines, options.sessionFilter ?? null);
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
