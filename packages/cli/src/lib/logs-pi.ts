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
        // One byte before the tail tells whether the tail begins on a line boundary.
        const probe = start > 0 ? 1 : 0;
        const buffer = Buffer.alloc(size - start + probe);
        // A read may return fewer bytes than asked; keep going until the tail is full or EOF.
        let filled = 0;
        while (filled < buffer.length) {
            const bytesRead = readSync(
                fd,
                buffer,
                filled,
                buffer.length - filled,
                start - probe + filled,
            );
            if (bytesRead === 0) break;
            filled += bytesRead;
        }
        const text = buffer.toString("utf-8", 0, filled);
        if (probe === 0) return text.split(/\r?\n/);
        if (text.startsWith("\n")) return text.slice(1).split(/\r?\n/);
        const lines = text.split(/\r?\n/);
        // The tail begins inside a line, so the first entry is a fragment.
        lines.shift();
        return lines;
    } finally {
        closeSync(fd);
    }
}

const TAG_PATTERN = /\[eidnara\]\[([^\]]+)\]/g;

/** `log` opens every entry with `[<ISO timestamp>]`; an `Error` stack continues on bare lines. */
const ENTRY_START_PATTERN = /^\[\d{4}-\d{2}-\d{2}T[^\]]*\]/;

/**
 * `sessionId` may end with `_<uuid>` because log tags contain bare UUIDs.
 * The filter keeps an entry only if it has tags and every tag matches `sessionId`.
 * Continuation lines retain the preceding entry's keep decision; lines before the first entry are excluded.
 */
function filterLogLinesBySession(lines: string[], sessionId: string | null): string[] {
    if (!sessionId) return lines;
    const isWanted = (tag: string) => tag === sessionId || sessionId.endsWith(`_${tag}`);
    let keep = false;
    return lines.filter((line) => {
        if (ENTRY_START_PATTERN.test(line)) {
            const tags = [...line.matchAll(TAG_PATTERN)].map((match) => match[1] ?? "");
            keep = tags.length > 0 && tags.every(isWanted);
        }
        return keep;
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
        renderDiagnosticsMarkdown(report),
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

const MAX_BUNDLE_NAME_ATTEMPTS = 100;

/**
 * The timestamp has one-second resolution, so a second bundle in the same
 * second takes a numbered suffix instead of replacing the first.
 */
function writeNewFile(stem: string, data: string): string {
    for (let attempt = 1; attempt <= MAX_BUNDLE_NAME_ATTEMPTS; attempt++) {
        const path = attempt === 1 ? `${stem}.md` : `${stem}-${attempt}.md`;
        try {
            // The bundle carries the user's description and raw log lines, so only the owner may read it.
            writeFileSync(path, data, { flag: "wx", mode: 0o600 });
            return path;
        } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        }
    }
    throw new Error(
        `Could not find a free bundle name after ${MAX_BUNDLE_NAME_ATTEMPTS} tries at ${stem}`,
    );
}
