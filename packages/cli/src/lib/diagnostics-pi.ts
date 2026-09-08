import { closeSync, existsSync, openSync, readdirSync, readSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { isAbsolute, join, parse } from "node:path";

import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
} from "@eidnara/opencode/config/config-paths";
import { getProjectEidnaraHistorianDir } from "@eidnara/opencode/shared/data-path";
import { detectConfigFile } from "@eidnara/opencode/shared/jsonc-parser";
import {
    escapeRegex,
    isSecretKey,
    redactSecretText,
    sanitizePathString,
} from "@eidnara/opencode/shared/redaction";
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
import { detectPiBinary, getPiVersion, matchesPiPackageSource } from "./pi-helpers";

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
        /** `null` when the environment provides no absolute home, so no user tier exists. */
        userConfig: string | null;
        projectConfig: string;
    };
    userConfig: PiConfigDiagnostic | null;
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
     * The raw session-slug folder name is the fallback when a header carries no `cwd`.
     */
    recentSessions: PiRecentSessionSummary[];
    /** The report keeps legacy tmp-dir dumps separate from project-grouped dumps. */
    historianDumps: PiHistorianDumpsReport;
}

export interface PiRecentSessionSummary {
    sessionId: string;
    /** The session header's `cwd`, or the raw session slug such as `--tmp-my-project--` when the header has none. */
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

/**
 * Text like `client_secret: value` is judged by the shared key vocabulary; booleans and
 * null stay, a number may be a PIN. A bare `key=` names a secret the way the shared text
 * redactor reads it, while `key:` keeps the prose carve-out (`press any key: continue`).
 */
const SIMPLE_ESCAPES: Record<string, string> = {
    n: "\n",
    r: "\r",
    t: "\t",
    b: "\b",
    f: "\f",
    v: "\v",
    "0": "\0",
};

/** Decodes JavaScript string escapes and removes line continuations. */
function decodeQuotedKey(raw: string): string {
    return raw.replace(
        /\\(?:(\r\n|[\n\r\u2028\u2029])|u\{([0-9A-Fa-f]{1,6})\}|u([0-9A-Fa-f]{4})|x([0-9A-Fa-f]{2})|(.))/g,
        (match, lineBreak?: string, braced?: string, u4?: string, x2?: string, ch?: string) => {
            if (lineBreak !== undefined) return "";
            if (braced !== undefined) {
                const codePoint = Number.parseInt(braced, 16);
                return codePoint <= 0x10ffff ? String.fromCodePoint(codePoint) : match;
            }
            if (u4 !== undefined) return String.fromCharCode(Number.parseInt(u4, 16));
            if (x2 !== undefined) return String.fromCharCode(Number.parseInt(x2, 16));
            return SIMPLE_ESCAPES[ch ?? ""] ?? ch ?? "";
        },
    );
}

const LINE_END_PATTERN = /\r?\n|$/g;

/** Index just past the bracket closing the structure opened at `start`; an unbalanced structure ends with its line. */
function structuredValueEnd(text: string, start: number): number {
    let depth = 0;
    for (let at = start; at < text.length; at++) {
        const ch = text[at];
        if (ch === '"' || ch === "'" || ch === "`") {
            for (at++; at < text.length && text[at] !== ch; at++) if (text[at] === "\\") at++;
        } else if (ch === "{" || ch === "[") {
            depth++;
        } else if (ch === "}" || ch === "]") {
            depth--;
            if (depth === 0) return at + 1;
        }
    }
    LINE_END_PATTERN.lastIndex = start;
    return LINE_END_PATTERN.exec(text)?.index ?? text.length;
}

// The key may be quoted, as in a JSON object literal: `{"password": 123456}`.
// Quoted keys may contain spaces, escapes, or backslash line continuations;
// a bare key is an identifier. A `{`/`[` value matches only its opening bracket.
const KEYED_VALUE_PATTERN =
    /(?:"((?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))+)"|'((?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))+)'|\b([A-Za-z][A-Za-z0-9_.-]*))(\s*[:=]\s*)("(?:[^"\\\r\n]|\\.)*"|'(?:[^'\\\r\n]|\\.)*'|[{[]|[^\s&;,}\]]+)/g;

function redactKeyedText(value: string): string {
    let out = "";
    let last = 0;
    KEYED_VALUE_PATTERN.lastIndex = 0;
    for (let m = KEYED_VALUE_PATTERN.exec(value); m !== null; m = KEYED_VALUE_PATTERN.exec(value)) {
        const [full, doubleQuoted, singleQuoted, bare, separator, secret] = m;
        const quote = doubleQuoted !== undefined ? '"' : singleQuoted !== undefined ? "'" : "";
        const rawKey = doubleQuoted ?? singleQuoted ?? bare ?? "";
        const key = quote ? decodeQuotedKey(rawKey) : rawKey;
        const bareKeyAssignment = !separator.includes(":") && /^keys?$/i.test(key);
        if (!(isSecretKey(key) || bareKeyAssignment) || /^(?:true|false|null)$/i.test(secret)) {
            continue;
        }
        // A structured value under a secret key is redacted whole, numeric-only contents
        // included, so a nested `pin` never reaches the bundle.
        const valueEnd = m.index + full.length;
        const end =
            secret === "{" || secret === "[" ? structuredValueEnd(value, valueEnd - 1) : valueEnd;
        // A quoted key marks a JSON literal, where a quoted placeholder keeps the document
        // well-formed and stops the shared redactor from reading past the value.
        const placeholder = quote ? `${quote}<REDACTED>${quote}` : "<REDACTED>";
        out += `${value.slice(last, m.index)}${quote}${rawKey}${quote}${separator}${placeholder}`;
        last = end;
        KEYED_VALUE_PATTERN.lastIndex = end;
    }
    return out + value.slice(last);
}

function redactSecretString(value: string): string {
    // The shared redactor treats an unquoted value as a YAML plain scalar through the next `: `,
    // so on one line it would swallow a neighboring `key: value` pair's key and leave that
    // pair's value in place. Redacting one-token pairs first removes that value.
    // Keep the local `sk-{12,}` redaction because `redactSecretText` only redacts `sk-` tokens with at least 32 characters.
    const redacted = redactSecretText(redactKeyedText(value))
        // Userinfo runs through the last `@` before the path so a raw `@` in a password is covered,
        // and a scheme-relative `//user@host` counts too.
        .replace(/((?:\b[a-z][a-z0-9+.-]*:)?\/\/)[^\s/?#]*@/gi, "$1<REDACTED>@")
        .replace(
            /(\b(?:Proxy-)?Authorization\s*[:=]\s*|\b(?:Set-)?Cookie\s*[:=]\s*|\bX-API-Key\s*[:=]\s*)[^\r\n]+/gi,
            "$1<REDACTED>",
        )
        .replace(/\bBearer\s+[A-Za-z0-9._~+\-/=]+/gi, "Bearer <REDACTED>")
        .replace(/sk-[A-Za-z0-9_-]{12,}/g, "sk-<REDACTED>");
    return redactKeyedText(redacted);
}

/**
 * `sanitizeString` redacts secret material, then the home directory, profile
 * paths, and account name, before issue reports are written.
 *
 * `HOME` from the environment is replaced first because the user config tier
 * is resolved from it, while the shared sanitizer reads the passwd home.
 */
export function sanitizeString(value: string): string {
    let sanitized = redactSecretString(value);
    const envHome = process.env.HOME?.trim();
    if (envHome && isAbsolute(envHome) && parse(envHome).root !== envHome) {
        sanitized = sanitized.replace(
            new RegExp(`${escapeRegex(envHome)}(?![A-Za-z0-9_.-])`, "g"),
            "~",
        );
    }
    return sanitizePathString(sanitized);
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
            Object.entries(value).map(([entryKey, entry]) => [
                sanitizeString(entryKey),
                redactProse(entry),
            ]),
        );
    }
    return value;
}

export function sanitizeValue(value: unknown, key = ""): unknown {
    // A boolean cannot carry a credential, but a number under a credential key can (a numeric PIN or token).
    if (value === null || typeof value === "boolean") return value;
    if (shouldRedactKey(key)) return "<REDACTED>";
    if (typeof value === "number") return value;
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
function getUserConfigPath(): string | null {
    const basePath = eidnaraUserConfigBasePath();
    return basePath === undefined ? null : detectConfigFile(basePath).path;
}

function getProjectConfigPath(cwd: string): string {
    return detectConfigFile(eidnaraProjectConfigBasePath(cwd)).path;
}

/**
 * The Pi loader reads its candidate files with a blocking `readFileSync`, so a
 * FIFO at any of them would stall diagnostics until a writer appeared. The
 * candidates are checked first and the loader is skipped when one is not a
 * regular file.
 */
function loadPiConfigUnlessBlocking(cwd: string): {
    loadedFromPaths: string[];
    warnings: string[];
} {
    const userBase = eidnaraUserConfigBasePath();
    const projectBase = eidnaraProjectConfigBasePath(cwd);
    // The loader opens only the first existing path of each tier, `.jsonc` before `.json`.
    const selected = [userBase, projectBase]
        .filter((base): base is string => base !== undefined)
        .map((base) => [`${base}.jsonc`, `${base}.json`].find((path) => existsSync(path)))
        .filter((path): path is string => path !== undefined);
    const blocking = selected.filter((path) => {
        try {
            const entry = statSync(path, { throwIfNoEntry: false });
            return entry !== undefined && !entry.isFile();
        } catch {
            return false;
        }
    });
    if (blocking.length === 0) return loadPiConfig({ cwd });
    return {
        loadedFromPaths: [],
        warnings: blocking.map(
            (path) => `${path}: not a regular file; the Pi config loader was not run`,
        ),
    };
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
 * Pi names a session directory after its project path: the leading `/` is
 * dropped, every `/` becomes `-`, and the result is wrapped in `--`. Literal
 * `-` characters in path components make that encoding lossy, so the slug is
 * never turned back into a path; it stands in as a label when the header
 * carries no `cwd`.
 */
function isSessionSlug(name: string): boolean {
    return name.startsWith("--") && name.endsWith("--");
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
        // A read may return fewer bytes than asked; keep going until the header line, the bound, or EOF.
        let filled = 0;
        let newline = -1;
        while (filled < buffer.length && newline === -1) {
            const bytesRead = readSync(fd, buffer, filled, buffer.length - filled, filled);
            if (bytesRead === 0) break;
            newline = buffer.indexOf(0x0a, filled);
            filled += bytesRead;
        }
        if (newline === -1 || newline >= filled) return null;
        const header = JSON.parse(buffer.toString("utf-8", 0, newline)) as { cwd?: unknown };
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
            slug: string;
            mtime: number;
        }> = [];

        for (const slug of slugs) {
            if (!isSessionSlug(slug)) continue;
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
                    if (!sessionId) continue;
                    candidates.push({ sessionId, file, slug, mtime: stat.mtimeMs });
                } catch {}
            }
        }

        candidates.sort((a, b) => b.mtime - a.mtime);
        return candidates.slice(0, 5).map((entry) => ({
            sessionId: entry.sessionId,
            directory: readSessionHeaderCwd(entry.file) ?? entry.slug,
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
        // A slug label is not a path, so no project directory is scanned for it.
        if (!isAbsolute(dir)) continue;
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
    const loaded = loadPiConfigUnlessBlocking(cwd);
    const logFile = statLogFile(getEidnaraLogPath("pi"));
    const otherPiExtensions = packages
        .filter((entry) => !matchesPiPackageSource(entry))
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
            hasEidnaraPackage: packages.some(matchesPiPackageSource),
            packages: sanitizeValue(packages) as unknown[],
        },
        configPaths: {
            agentDir: getPiAgentDir(),
            userConfig: userConfigPath,
            projectConfig: projectConfigPath,
        },
        userConfig: userConfigPath === null ? null : readConfigDiagnostic(userConfigPath),
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
        `- User config parse error: ${oneLine(report.userConfig?.parseError ?? "none")}`,
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
        JSON.stringify(report.userConfig?.flags ?? null, null, 2),
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
