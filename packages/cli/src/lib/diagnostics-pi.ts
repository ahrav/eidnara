import { createHash } from "node:crypto";
import { closeSync, existsSync, openSync, readdirSync, readSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir, userInfo } from "node:os";
import { join } from "node:path";

import { resolveEidnaraProjectConfigPath } from "@eidnara/opencode/config/config-paths";
import { getProjectEidnaraHistorianDir } from "@eidnara/opencode/shared/data-path";
import { escapeRegex, redactSecretText } from "@eidnara/opencode/shared/redaction";
import { loadPiConfig } from "@eidnara/pi/config";
import {
    type HistorianDumpMeta,
    type HistorianDumpSummary,
    listDumpsInDir,
} from "./historian-dumps";
import { readJsoncLenient } from "./jsonc-config";
import {
    getEidnaraHistorianDir,
    getEidnaraLogPath,
    getPiAgentDir,
    getPiSessionsRoot,
    getPiUserExtensionsPath,
    getSharedUserConfigPath,
} from "./paths";
import { detectPiBinary, getPiVersion, isEidnaraPiPackageEntry } from "./pi-helpers";
import { standaloneVersion } from "./semver";

/** Pi-named aliases of the shared historian-dump shapes. */
export type PiHistorianDumpMeta = HistorianDumpMeta;
export type PiHistorianDumpSummary = HistorianDumpSummary;

export interface PiConfigDiagnostic {
    path: string;
    exists: boolean;
    parseError?: string;
    flags: Record<string, unknown>;
}

export interface PiDiagnosticReport {
    timestamp: string;
    platform: string;
    arch: string;
    nodeVersion: string;
    pluginVersion: string;
    piInstalled: boolean;
    piPath: string | null;
    piVersion: string | null;
    settings: {
        path: string;
        exists: boolean;
        parseError?: string;
        hasEidnaraPackage: boolean;
        packages: unknown[];
    };
    configPaths: {
        agentDir: string;
        userConfig: string;
        projectConfig: string;
    };
    userConfig: PiConfigDiagnostic;
    projectConfig: PiConfigDiagnostic;
    loadedConfigPaths: string[];
    loadWarnings: string[];
    conflicts: {
        knownConflicts: string[];
        otherPiExtensions: string[];
    };
    logFile: {
        path: string;
        exists: boolean;
        sizeKb: number;
    };
    /**
     * `recentSessions` contains the five JSONL sessions with the newest mtimes.
     * `--issue` uses these sessions in its session picker.
     * Pi stores session JSONL files under `~/.pi/agent/sessions/<slug>/*.jsonl`.
     * Pi derives each session slug by replacing `/` in the project directory with `-`.
     * Pi wraps each session slug in `--`.
     */
    recentSessions: PiRecentSessionSummary[];
    /**
     * `unavailable` when the sessions directory exists but could not be read,
     * so an empty `recentSessions` is a failure, not an absence.
     */
    sessionDiscovery: PiSessionDiscovery["status"];
    /** The report keeps legacy tmp-dir dumps separate from project-grouped dumps. */
    historianDumps: PiHistorianDumpsReport;
}

export interface PiRecentSessionSummary {
    /**
     * Pi's own session id: the value `sessionManager.getSessionId()` returns
     * and the plugin writes into each `[eidnara][<id>]` log line. Pi names the
     * session file `<timestamp>_<id>.jsonl`; the timestamp prefix is not part
     * of `sessionId`.
     */
    sessionId: string;
    /** Pi's session-slug folder determines `directory`. */
    directory: string;
    /** The JSONL file's mtime determines `lastActiveAt` in ISO format. */
    lastActiveAt: string;
}

export interface PiProjectHistorianBucket {
    directory: string;
    primarySessionId: string;
    sessionIds: string[];
    count: number;
    recent: PiHistorianDumpSummary[];
}

export interface PiHistorianDumpsReport {
    byProject: PiProjectHistorianBucket[];
    legacyDumps: {
        dir: string;
        count: number;
        recent: PiHistorianDumpSummary[];
    };
}

function getSelfVersion(): string {
    const req = createRequire(import.meta.url);
    for (const relPath of ["../../package.json", "../package.json"]) {
        try {
            const pkg = req(relPath) as { version?: unknown };
            if (typeof pkg.version === "string" && pkg.version.length > 0) {
                return pkg.version;
            }
        } catch {
            // The fallback supports both source and bundled layouts.
        }
    }
    return "unknown";
}

function currentUserHash(): string {
    const username = userInfo().username || "unknown";
    return createHash("sha256").update(username).digest("hex").slice(0, 12);
}

function redactSecretString(value: string): string {
    // Keep the local `sk-{12,}` redaction because `redactSecretText` only redacts `sk-` tokens with at least 32 characters.
    return redactSecretText(value)
        .replace(/Bearer\s+[A-Za-z0-9._~+\-/=]+/g, "Bearer <REDACTED>")
        .replace(/sk-[A-Za-z0-9_-]{12,}/g, "sk-<REDACTED>")
        .replace(/api[_-]?key=([^\s&]+)/gi, "api_key=<REDACTED>")
        .replace(/token=([^\s&]+)/gi, "token=<REDACTED>");
}

/**
 * `sanitizeString` redacts paths, usernames, and secret material before issue reports are written.
 * `sanitizeString` replaces the exact home path with `<HOME>`.
 * `sanitizeString` replaces the local username with a stable short hash.
 * The stable hash correlates repeated occurrences without exposing the account name.
 */
export function sanitizeString(value: string): string {
    const home = process.env.HOME || homedir();
    const username = userInfo().username;
    const userHash = `<USER:${currentUserHash()}>`;
    let sanitized = redactSecretString(value);
    if (home) {
        sanitized = sanitized.replace(new RegExp(escapeRegex(home), "g"), "<HOME>");
    }
    sanitized = sanitized.replace(/\/Users\/[^/]+\//g, `/Users/${userHash}/`);
    sanitized = sanitized.replace(/\/home\/[^/]+\//g, `/home/${userHash}/`);
    sanitized = sanitized.replace(/C:\\Users\\[^\\]+\\/g, `C:\\Users\\${userHash}\\`);
    if (username) {
        sanitized = sanitized.replace(new RegExp(escapeRegex(username), "g"), userHash);
    }
    return sanitized;
}

function shouldRedactKey(key: string): boolean {
    return /api[_-]?key|token|secret|password|authorization|cookie/i.test(key);
}

export function sanitizeValue(value: unknown, key = ""): unknown {
    if (value === null || typeof value === "number" || typeof value === "boolean") return value;
    if (shouldRedactKey(key)) return "<REDACTED>";
    if (typeof value === "string") return sanitizeString(value);
    if (Array.isArray(value)) return value.map((entry) => sanitizeValue(entry));
    if (value && typeof value === "object") {
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [
                entryKey,
                sanitizeValue(entry, entryKey),
            ]),
        );
    }
    return value;
}

function getProjectConfigPath(cwd: string): string {
    return resolveEidnaraProjectConfigPath(cwd);
}

function readConfigDiagnostic(path: string): PiConfigDiagnostic {
    const parsed = readJsoncLenient(path);
    return {
        path,
        exists: existsSync(path),
        ...(parsed.parseError ? { parseError: sanitizeString(parsed.parseError) } : {}),
        flags: sanitizeValue(parsed.value) as Record<string, unknown>,
    };
}

function packageEntries(settings: Record<string, unknown>): unknown[] {
    return Array.isArray(settings.packages) ? settings.packages : [];
}

function describePackageEntry(entry: unknown): string {
    if (typeof entry === "string") return entry;
    if (entry && typeof entry === "object") {
        const { name, source } = entry as { name?: unknown; source?: unknown };
        if (typeof name === "string") return name;
        if (typeof source === "string") return source;
    }
    return String(entry);
}

/**
 * A session-slug directory encodes its source project path.
 *
 * Pi strips the leading `/`, replaces `/` with `-`, and wraps the result in `--`.
 *
 * Literal `-` characters in path components make session-slug reversal lossy,
 * so the reversal is only a fallback for a session file whose header lacks `cwd`.
 */
function reverseSlugToDirectory(slug: string): string | null {
    if (!slug.startsWith("--") || !slug.endsWith("--")) return null;
    const inner = slug.slice(2, -2);
    if (!inner) return null;
    return `/${inner.replace(/-/g, "/")}`;
}

/** A Pi session header is one JSON line; 4 KiB covers any path it can carry. */
const SESSION_HEADER_BYTES = 4096;

/**
 * Pi writes `{"type":"session", ..., "cwd": <project directory>}` as the first
 * line of every session file. The header's `cwd` is the exact directory, unlike
 * the slug, so it is read from the top of the file; a missing or malformed
 * header yields `null`.
 */
function readSessionHeaderCwd(path: string): string | null {
    const buffer = Buffer.alloc(SESSION_HEADER_BYTES);
    const fd = openSync(path, "r");
    let read = 0;
    try {
        read = readSync(fd, buffer, 0, buffer.length, 0);
    } finally {
        closeSync(fd);
    }
    const text = buffer.toString("utf-8", 0, read);
    const newline = text.indexOf("\n");
    if (newline === -1) return null;
    try {
        const header = JSON.parse(text.slice(0, newline)) as { type?: unknown; cwd?: unknown };
        return header.type === "session" && typeof header.cwd === "string" && header.cwd
            ? header.cwd
            : null;
    } catch {
        return null;
    }
}

/**
 * Pi names each session file `<timestamp>_<sessionId>.jsonl`, where the
 * timestamp is an ISO-8601 instant with `:` and `.` replaced by `-`. The id may
 * itself contain `_`, so only a leading timestamp of that exact shape is
 * stripped; a file without one is treated as a bare id.
 */
const PI_SESSION_FILE_TIMESTAMP_PREFIX = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_/;

export function piSessionIdFromFileName(fileName: string): string {
    return fileName.replace(/\.jsonl$/, "").replace(PI_SESSION_FILE_TIMESTAMP_PREFIX, "");
}

/**
 * Pi stores session JSONL files under `~/.pi/agent/sessions/<slug>/*.jsonl`.
 *
 * The session reader returns an empty array when `~/.pi/agent/sessions/` does not exist.
 */
export type PiSessionDiscovery =
    | { status: "ok" | "partial"; sessions: PiRecentSessionSummary[] }
    | { status: "unavailable"; sessions: [] };

/**
 * OMP keeps the same `<slug>/<timestamp>_<id>.jsonl` layout under its own
 * root, so the OMP doctor passes `getOmpSessionsRoot()`.
 */
export function collectPiRecentSessions(
    sessionsRoot: string = getPiSessionsRoot(),
): PiSessionDiscovery {
    if (!existsSync(sessionsRoot)) return { status: "ok", sessions: [] };
    try {
        const slugs = readdirSync(sessionsRoot, { withFileTypes: true })
            .filter((entry) => entry.isDirectory())
            .map((entry) => entry.name);

        const candidates: Array<{
            sessionId: string;
            path: string;
            slugDirectory: string;
            mtime: number;
        }> = [];

        // A per-directory permission error must not read as "no sessions".
        let unreadable = 0;
        for (const slug of slugs) {
            const slugDirectory = reverseSlugToDirectory(slug);
            if (!slugDirectory) continue;
            const slugDir = join(sessionsRoot, slug);
            let files: string[];
            try {
                files = readdirSync(slugDir).filter((name) => name.endsWith(".jsonl"));
            } catch {
                unreadable += 1;
                continue;
            }
            for (const file of files) {
                try {
                    const path = join(slugDir, file);
                    const mtime = statSync(path).mtimeMs;
                    const sessionId = piSessionIdFromFileName(file);
                    candidates.push({ sessionId, path, slugDirectory, mtime });
                } catch {
                    unreadable += 1;
                }
            }
        }
        if (candidates.length === 0 && unreadable > 0) {
            return { status: "unavailable", sessions: [] };
        }

        candidates.sort((a, b) => b.mtime - a.mtime);
        // Headers are read only for the sessions that are reported.
        const sessions = candidates.slice(0, 5).map((entry) => {
            let directory = entry.slugDirectory;
            try {
                directory = readSessionHeaderCwd(entry.path) ?? directory;
            } catch {}
            return {
                sessionId: entry.sessionId,
                directory,
                lastActiveAt: new Date(entry.mtime).toISOString(),
            };
        });
        // Some sessions were read while others were not, so the list is
        // incomplete rather than empty.
        return { status: unreadable > 0 ? "partial" : "ok", sessions };
    } catch {
        return { status: "unavailable", sessions: [] };
    }
}

export function collectPiHistorianDumps(
    recentSessions: PiRecentSessionSummary[],
): PiHistorianDumpsReport {
    const buckets = new Map<string, PiProjectHistorianBucket>();
    for (const session of recentSessions) {
        const dir = session.directory;
        if (!dir) continue;
        const projectHistorianDir = getProjectEidnaraHistorianDir(dir);
        const listing = listDumpsInDir(projectHistorianDir, 5);
        const existing = buckets.get(dir);
        if (existing) {
            if (!existing.sessionIds.includes(session.sessionId)) {
                existing.sessionIds.push(session.sessionId);
            }
            continue;
        }
        if (listing.count === 0) continue;
        buckets.set(dir, {
            directory: dir,
            primarySessionId: session.sessionId,
            sessionIds: [session.sessionId],
            count: listing.count,
            recent: listing.recent,
        });
    }

    const legacyDir = getEidnaraHistorianDir("pi");
    const legacyListing = listDumpsInDir(legacyDir, 5);

    return {
        byProject: [...buckets.values()],
        legacyDumps: {
            dir: legacyDir,
            count: legacyListing.count,
            recent: legacyListing.recent,
        },
    };
}

export async function collectDiagnostics(cwd = process.cwd()): Promise<PiDiagnosticReport> {
    const pi = detectPiBinary();
    const settingsPath = getPiUserExtensionsPath();
    const settingsParsed = readJsoncLenient(settingsPath);
    const packages = packageEntries(settingsParsed.value);
    const userConfigPath = getSharedUserConfigPath();
    const projectConfigPath = getProjectConfigPath(cwd);
    const loaded = loadPiConfig({ cwd });
    const logPath = getEidnaraLogPath("pi");
    const logFileSize = existsSync(logPath) ? statSync(logPath).size : 0;
    const otherPiExtensions = packages
        .filter((entry) => !isEidnaraPiPackageEntry(entry, getPiAgentDir()))
        .map(describePackageEntry);
    const discovery = collectPiRecentSessions();
    const recentSessions = discovery.sessions;
    const historianDumps = collectPiHistorianDumps(recentSessions);

    return {
        timestamp: new Date().toISOString(),
        platform: process.platform,
        arch: process.arch,
        nodeVersion: process.version,
        pluginVersion: getSelfVersion(),
        piInstalled: pi !== null,
        piPath: pi?.path ?? null,
        // Only the parsed semver enters the report; `pi --version` output can
        // carry warnings that name paths or credentials.
        piVersion: pi ? standaloneVersion(getPiVersion(pi.path)) : null,
        settings: {
            path: settingsPath,
            exists: existsSync(settingsPath),
            ...(settingsParsed.parseError ? { parseError: settingsParsed.parseError } : {}),
            hasEidnaraPackage: packages.some((entry) =>
                isEidnaraPiPackageEntry(entry, getPiAgentDir()),
            ),
            packages: sanitizeValue(packages) as unknown[],
        },
        configPaths: {
            agentDir: getPiAgentDir(),
            userConfig: userConfigPath,
            projectConfig: projectConfigPath,
        },
        userConfig: readConfigDiagnostic(userConfigPath),
        projectConfig: readConfigDiagnostic(projectConfigPath),
        loadedConfigPaths: loaded.loadedFromPaths.map(sanitizeString),
        loadWarnings: loaded.warnings.map(sanitizeString),
        conflicts: {
            knownConflicts: [],
            otherPiExtensions: otherPiExtensions.map(sanitizeString),
        },
        logFile: {
            path: logPath,
            exists: existsSync(logPath),
            sizeKb: Math.round(logFileSize / 1024),
        },
        recentSessions,
        sessionDiscovery: discovery.status,
        historianDumps,
    };
}

export function renderDiagnosticsMarkdown(report: PiDiagnosticReport): string {
    const configPaths = sanitizeValue(report.configPaths);
    const settings = sanitizeValue(report.settings);

    return [
        `- Timestamp: ${report.timestamp}`,
        `- Pi plugin: v${report.pluginVersion}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        `- Pi installed: ${report.piInstalled}${report.piVersion ? ` (${report.piVersion})` : ""}`,
        `- Eidnara package registered: ${report.settings.hasEidnaraPackage}`,
        `- User config parse error: ${report.userConfig.parseError ?? "none"}`,
        `- Project config parse error: ${report.projectConfig.parseError ?? "none"}`,
        `- Known Pi extension conflicts: ${report.conflicts.knownConflicts.length === 0 ? "none" : report.conflicts.knownConflicts.join("; ")}`,
        "",
        "### Pi settings",
        "```json",
        JSON.stringify(settings, null, 2),
        "```",
        "",
        "### Config paths",
        "```json",
        JSON.stringify(configPaths, null, 2),
        "```",
        "",
        "### User eidnara.jsonc flags",
        "```jsonc",
        JSON.stringify(report.userConfig.flags, null, 2),
        "```",
        "",
        "### Project eidnara.jsonc flags",
        "```jsonc",
        JSON.stringify(report.projectConfig.flags, null, 2),
        "```",
        "",
        "### Loaded config paths",
        report.loadedConfigPaths.length === 0
            ? "_No config files loaded; defaults are in use._"
            : report.loadedConfigPaths.map((path) => `- ${path}`).join("\n"),
        "",
        "### Config load warnings",
        report.loadWarnings.length === 0
            ? "_None._"
            : report.loadWarnings.map((warning) => `- ${warning}`).join("\n"),
        "",
        "### Pi extension conflicts",
        "No known conflicting Pi extensions are currently registered. Other Pi packages are informational only.",
        "```json",
        JSON.stringify(report.conflicts, null, 2),
        "```",
        "",
        "### Log file",
        `- Path: ${sanitizeString(report.logFile.path)}`,
        `- Exists: ${report.logFile.exists}`,
        `- Size: ${report.logFile.sizeKb} KB`,
    ].join("\n");
}
