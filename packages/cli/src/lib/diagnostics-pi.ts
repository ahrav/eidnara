import { closeSync, existsSync, openSync, readdirSync, readSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir, userInfo } from "node:os";
import { basename, isAbsolute, join, parse } from "node:path";

import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
} from "@eidnara/opencode/config/config-paths";
import { getProjectEidnaraHistorianDir } from "@eidnara/opencode/shared/data-path";
import { detectConfigFile } from "@eidnara/opencode/shared/jsonc-parser";
import { escapeRegex, isSecretKey, redactSecretText } from "@eidnara/opencode/shared/redaction";
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
} from "./paths";
import { detectPiBinary, getPiVersion, PI_PACKAGE_SOURCE } from "./pi-helpers";

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
     * Each session's `directory` comes from the `cwd` in its JSONL header line.
     * The session-slug folder is the fallback when a header carries no `cwd`.
     */
    recentSessions: PiRecentSessionSummary[];
    /** The report keeps legacy tmp-dir dumps separate from project-grouped dumps. */
    historianDumps: PiHistorianDumpsReport;
}

export interface PiRecentSessionSummary {
    sessionId: string;
    /** The session header's `cwd`, or the reversed session slug when the header has none. */
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

function currentHome(): string {
    if (process.env.HOME) return process.env.HOME;
    try {
        return homedir();
    } catch {
        return "";
    }
}

function currentUsername(): string | undefined {
    try {
        const username = userInfo().username;
        if (username) return username;
    } catch {}
    const home = currentHome();
    const fromHome = home ? basename(home) : "";
    return fromHome || undefined;
}

/** Text like `client_secret: value` is judged by the shared key vocabulary; numbers and booleans stay. */
function redactKeyedText(value: string): string {
    return value.replace(
        /\b([A-Za-z][A-Za-z0-9_.-]*)(\s*[:=]\s*)("[^"\r\n]*"|'[^'\r\n]*'|[^\s&;,]+)/g,
        (full, key: string, separator: string, secret: string) =>
            isSecretKey(key) && !/^(?:true|false|null|[+-]?\d+(?:\.\d+)?)$/i.test(secret)
                ? `${key}${separator}<REDACTED>`
                : full,
    );
}

function redactSecretString(value: string): string {
    // Keep the local `sk-{12,}` redaction because `redactSecretText` only redacts `sk-` tokens with at least 32 characters.
    const redacted = redactSecretText(value)
        .replace(/-----BEGIN [A-Z ]+-----[\s\S]*?(?:-----END [A-Z ]+-----|$)/g, "<REDACTED PEM>")
        .replace(/(\b[a-z][a-z0-9+.-]*:\/\/)[^\s/@]+@/gi, "$1<REDACTED>@")
        .replace(
            /(\b(?:Proxy-)?Authorization\s*[:=]\s*|\b(?:Set-)?Cookie\s*[:=]\s*|\bX-API-Key\s*[:=]\s*)[^\r\n]+/gi,
            "$1<REDACTED>",
        )
        .replace(/\bBearer\s+[A-Za-z0-9._~+\-/=]+/gi, "Bearer <REDACTED>")
        .replace(/sk-[A-Za-z0-9_-]{12,}/g, "sk-<REDACTED>");
    return redactKeyedText(redacted);
}

/**
 * `sanitizeString` redacts paths, usernames, and secret material before issue reports are written.
 * Home roots are not redacted because they would match every path separator.
 */
export function sanitizeString(value: string): string {
    const home = currentHome();
    const username = currentUsername();
    // Windows paths compare case-insensitively, so home and account matches do too there.
    const flags = process.platform === "win32" ? "gi" : "g";
    let sanitized = redactSecretString(value);
    if (home && parse(home).root !== home) {
        sanitized = sanitized.replace(new RegExp(escapeRegex(home), flags), "<HOME>");
    }
    sanitized = sanitized.replace(/(\/Users\/)[^/\s"'`]+/gi, "$1<USER>");
    sanitized = sanitized.replace(/(\/home\/)[^/\s"'`]+/gi, "$1<USER>");
    sanitized = sanitized.replace(/[A-Za-z]:[\\/]Users[\\/][^\\/\s"'`]+/gi, "C:\\Users\\<USER>");
    if (username) {
        sanitized = sanitized.replace(new RegExp(escapeRegex(username), flags), "<USER>");
    }
    return sanitized;
}

function shouldRedactKey(key: string): boolean {
    return isSecretKey(key) || /cookie/i.test(key);
}

/** Prompt fields hold arbitrary private prose; the report keeps only presence and length. */
function isPromptKey(key: string): boolean {
    return /^(prompt|system_prompt|description|tool_descriptions|skip_signatures)$/.test(key);
}

function redactProse(value: unknown): unknown {
    if (typeof value === "string") return `<REDACTED ${value.length} chars>`;
    if (Array.isArray(value)) return value.map(redactProse);
    if (value && typeof value === "object") {
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [entryKey, redactProse(entry)]),
        );
    }
    return value;
}

export function sanitizeValue(value: unknown, key = ""): unknown {
    if (value === null || typeof value === "number" || typeof value === "boolean") return value;
    if (shouldRedactKey(key)) return "<REDACTED>";
    if (isPromptKey(key)) return redactProse(value);
    if (typeof value === "string") return sanitizeString(value);
    if (Array.isArray(value)) return value.map((entry) => sanitizeValue(entry));
    if (value && typeof value === "object") {
        // Dynamic-key records such as `permission.bash` carry user text in the key itself.
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [
                sanitizeString(entryKey),
                sanitizeValue(entry, entryKey),
            ]),
        );
    }
    return value;
}

/**
 * `detectConfigFile` applies the Pi loader's precedence: `.jsonc`, then `.json`,
 * and the `.jsonc` path when neither exists.
 */
function getUserConfigPath(): string {
    return detectConfigFile(eidnaraUserConfigBasePath()).path;
}

function getProjectConfigPath(cwd: string): string {
    return detectConfigFile(eidnaraProjectConfigBasePath(cwd)).path;
}

/** Parse errors carry the absolute config path, so they are sanitized like any other path. */
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
 * so the slug is the fallback when the header does not provide an absolute `cwd`.
 */
function reverseSlugToDirectory(slug: string): string | null {
    if (!slug.startsWith("--") || !slug.endsWith("--")) return null;
    const inner = slug.slice(2, -2);
    return `/${inner.replace(/-/g, "/")}`;
}

const SESSION_HEADER_MAX_BYTES = 8 * 1024;

/**
 * The first JSONL line is Pi's session header, whose `cwd` is the exact project
 * path. The read is bounded because the rest of the file is the transcript.
 */
function readSessionHeaderCwd(file: string): string | null {
    let fd: number | null = null;
    try {
        fd = openSync(file, "r");
        const buffer = Buffer.alloc(SESSION_HEADER_MAX_BYTES);
        const bytesRead = readSync(fd, buffer, 0, buffer.length, 0);
        const text = buffer.toString("utf-8", 0, bytesRead);
        const newline = text.indexOf("\n");
        if (newline === -1) return null;
        const header = JSON.parse(text.slice(0, newline)) as { cwd?: unknown };
        return typeof header.cwd === "string" && isAbsolute(header.cwd) ? header.cwd : null;
    } catch {
        return null;
    } finally {
        if (fd !== null) closeSync(fd);
    }
}

/**
 * Pi stores session JSONL files under `~/.pi/agent/sessions/<slug>/*.jsonl`.
 *
 * The session reader returns an empty array when `~/.pi/agent/sessions/` does not exist.
 */
function collectPiRecentSessions(): PiRecentSessionSummary[] {
    const sessionsRoot = getPiSessionsRoot();
    if (!existsSync(sessionsRoot)) return [];
    try {
        const slugs = readdirSync(sessionsRoot, { withFileTypes: true })
            .filter((entry) => entry.isDirectory())
            .map((entry) => entry.name);

        const candidates: Array<{
            sessionId: string;
            file: string;
            slugDirectory: string;
            mtime: number;
        }> = [];

        for (const slug of slugs) {
            const slugDirectory = reverseSlugToDirectory(slug);
            if (!slugDirectory) continue;
            const slugDir = join(sessionsRoot, slug);
            let files: string[];
            try {
                files = readdirSync(slugDir).filter((name) => name.endsWith(".jsonl"));
            } catch {
                continue;
            }
            for (const name of files) {
                const file = join(slugDir, name);
                try {
                    const stat = statSync(file);
                    // Opening a FIFO with no writer blocks, so only a regular file is a candidate.
                    if (!stat.isFile()) continue;
                    const sessionId = name.replace(/\.jsonl$/, "");
                    candidates.push({ sessionId, file, slugDirectory, mtime: stat.mtimeMs });
                } catch {}
            }
        }

        candidates.sort((a, b) => b.mtime - a.mtime);
        return candidates.slice(0, 5).map((entry) => ({
            sessionId: entry.sessionId,
            directory: readSessionHeaderCwd(entry.file) ?? entry.slugDirectory,
            lastActiveAt: new Date(entry.mtime).toISOString(),
        }));
    } catch {
        return [];
    }
}

function collectPiHistorianDumps(recentSessions: PiRecentSessionSummary[]): PiHistorianDumpsReport {
    const buckets = new Map<string, PiProjectHistorianBucket>();
    for (const session of recentSessions) {
        const dir = session.directory;
        if (!dir) continue;
        const existing = buckets.get(dir);
        if (existing) {
            if (!existing.sessionIds.includes(session.sessionId)) {
                existing.sessionIds.push(session.sessionId);
            }
            continue;
        }
        const listing = listDumpsInDir(getProjectEidnaraHistorianDir(dir), 5);
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

/**
 * One `stat` decides both fields, so a file removed or made unreadable between
 * two probes cannot abort the report; it reads as absent.
 */
function statLogFile(path: string): PiDiagnosticReport["logFile"] {
    try {
        const stat = statSync(path);
        // Opening a FIFO with no writer blocks, so only a regular file counts as readable.
        if (!stat.isFile()) return { path, exists: false, sizeKb: 0 };
        return { path, exists: true, sizeKb: Math.round(stat.size / 1024) };
    } catch {
        return { path, exists: false, sizeKb: 0 };
    }
}

function sanitizeOptional(value: string | null): string | null {
    return value === null ? null : oneLine(sanitizeString(value));
}

export async function collectDiagnostics(cwd = process.cwd()): Promise<PiDiagnosticReport> {
    const pi = detectPiBinary();
    const settingsPath = getPiUserExtensionsPath();
    const settingsParsed = readJsoncLenient(settingsPath);
    const packages = packageEntries(settingsParsed.value);
    const userConfigPath = getUserConfigPath();
    const projectConfigPath = getProjectConfigPath(cwd);
    const loaded = loadPiConfig({ cwd });
    const logFile = statLogFile(getEidnaraLogPath("pi"));
    const otherPiExtensions = packages
        .filter((entry) => entry !== PI_PACKAGE_SOURCE)
        .map(describePackageEntry);
    const recentSessions = collectPiRecentSessions();
    const historianDumps = collectPiHistorianDumps(recentSessions);

    return {
        timestamp: new Date().toISOString(),
        platform: process.platform,
        arch: process.arch,
        nodeVersion: process.version,
        pluginVersion: getSelfVersion(),
        piInstalled: pi !== null,
        piPath: pi?.path ?? null,
        piVersion: pi ? sanitizeOptional(getPiVersion(pi.path)) : null,
        settings: {
            path: settingsPath,
            exists: existsSync(settingsPath),
            ...(settingsParsed.parseError
                ? { parseError: sanitizeString(settingsParsed.parseError) }
                : {}),
            hasEidnaraPackage: packages.some((entry) => entry === PI_PACKAGE_SOURCE),
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
        logFile,
        recentSessions,
        historianDumps,
    };
}

/**
 * Strings rendered outside a fenced block stay on one line so a newline inside
 * a config key or parser message cannot inject a Markdown heading.
 */
function oneLine(value: string): string {
    return value.replace(/\s*[\r\n]+\s*/g, " ");
}

export function renderDiagnosticsMarkdown(report: PiDiagnosticReport): string {
    const configPaths = sanitizeValue(report.configPaths);
    const settings = sanitizeValue(report.settings);

    return [
        `- Timestamp: ${report.timestamp}`,
        `- Pi plugin: v${report.pluginVersion}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        `- Pi installed: ${report.piInstalled}${report.piVersion ? ` (${oneLine(report.piVersion)})` : ""}`,
        `- Eidnara package registered: ${report.settings.hasEidnaraPackage}`,
        `- User config parse error: ${oneLine(report.userConfig.parseError ?? "none")}`,
        `- Project config parse error: ${oneLine(report.projectConfig.parseError ?? "none")}`,
        `- Known Pi extension conflicts: ${report.conflicts.knownConflicts.length === 0 ? "none" : oneLine(report.conflicts.knownConflicts.join("; "))}`,
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
            : report.loadedConfigPaths.map((path) => `- ${oneLine(path)}`).join("\n"),
        "",
        "### Config load warnings",
        report.loadWarnings.length === 0
            ? "_None._"
            : report.loadWarnings.map((warning) => `- ${oneLine(warning)}`).join("\n"),
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
