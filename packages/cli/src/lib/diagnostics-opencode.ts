// A static `import { Database } from "bun:sqlite"` crashes the Node CLI before `try/catch` can run.
// Node's ESM loader rejects `bun:` specifiers during resolution.
// If the DB cannot be read, the report still includes all other diagnostics.
import { existsSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { loadPluginConfig } from "@eidnara/opencode/config";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
} from "@eidnara/opencode/config/config-paths";
import {
    type ConflictResult,
    detectConflicts,
    projectConfigDisabled,
    projectOpenCodeConfigPaths,
} from "@eidnara/opencode/shared/conflict-detector";
import { getDataDir, getProjectEidnaraHistorianDir } from "@eidnara/opencode/shared/data-path";
import { detectConfigFile } from "@eidnara/opencode/shared/jsonc-parser";
import { resolveOpenCodeDatabaseCandidates } from "@eidnara/opencode/shared/opencode-database-path";
import {
    describeProseLength,
    sanitizeConfigValue,
    sanitizeDiagnosticText,
} from "@eidnara/opencode/shared/redaction";
import { readRegularFileSync } from "@eidnara/opencode/shared/regular-file";
import { parse as parseJsonc } from "comment-json";
import { isDevPathPluginEntry, matchesPluginEntry } from "../adapters/opencode";
import { type HistorianDumpSummary, listDumpsInDir } from "./historian-dumps";
import { codeFenceFor } from "./issue-body";
import { detectOpenCodeInstallations } from "./opencode-detect";
import { describeOpenCodeInstallations, type OpenCodeInstallationReport } from "./opencode-helpers";
import {
    type ConfigPaths,
    detectConfigPaths,
    getEidnaraHistorianDir,
    getEidnaraLogPath,
} from "./paths";

export type { HistorianDumpMeta, HistorianDumpSummary } from "./historian-dumps";

const OPENCODE_PLUGIN_NAME = "@eidnara/opencode";

/**
 * One Eidnara config tier as the plugin loader resolves it: `.jsonc` first, then `.json`.
 * `path` is the detected file, or the canonical `.jsonc` path when neither exists.
 */
export interface EidnaraConfigTier {
    path: string;
    exists: boolean;
    parseError?: string;
    flags: Record<string, unknown>;
}

export interface ProjectOpenCodeConfigReport {
    /** Existing `<cwd>/.opencode/opencode.json(c)` and `<cwd>/opencode.json(c)` files, in OpenCode's load order. */
    paths: string[];
    /** True when any listed file registers the plugin. */
    hasPlugin: boolean;
    parseErrors: string[];
}

export interface DiagnosticReport {
    timestamp: string;
    platform: string;
    arch: string;
    nodeVersion: string;
    pluginVersion: string;
    opencodeInstalled: boolean;
    opencodeInstallKind: "cli" | "desktop" | "none";
    opencodeVersion: string | null;
    /** `opencodeInstallations` marks the first detection-ladder rung as active. */
    opencodeInstallations: OpenCodeInstallationReport[];
    configPaths: ConfigPaths;
    /** Set when user-level paths could not be resolved (no `HOME`, no `XDG_CONFIG_HOME`, no passwd entry); the paths are then empty. */
    configPathsError?: string;
    /** Project-tier fields were collected for this directory; bundles for another directory must be re-collected. */
    projectDirectory: string;
    /** Registration in the user-level `opencode.json(c)` under the OpenCode config dir. */
    opencodeConfigHasPlugin: boolean;
    /** A malformed or unreadable `opencode.json(c)` reports `false` for `opencodeConfigHasPlugin`; the error explains why. */
    opencodeConfigParseError?: string;
    tuiConfigHasPlugin: boolean;
    tuiConfigParseError?: string;
    projectOpencodeConfig: ProjectOpenCodeConfigReport;
    /** User tier under `$XDG_CONFIG_HOME/eidnara/`. */
    eidnaraConfig: EidnaraConfigTier;
    /** Project tier under `<cwd>/.eidnara/`; its overrides win over the user tier. */
    projectConfig: EidnaraConfigTier;
    conflicts: {
        hasConflict: boolean;
        reasons: string[];
        /** `compactionEnabled` stores the resolved Eidnara compaction mode used by the writer and fixer. */
        compactionEnabled: boolean;
        /** `nativeCompaction` stores the resolved native OpenCode `auto` and `prune` states. */
        nativeCompaction: {
            auto: boolean;
            prune: boolean;
        };
        /** Set when conflict detection itself failed; `hasConflict` is then `false` by default, not by evidence. */
        detectionError?: string;
    };
    logFile: {
        path: string;
        exists: boolean;
        sizeKb: number;
    };
    /**
     * `recentSessions` contains the five most recently updated active OpenCode sessions.
     * `recentSessions` supplies session choices for the `--issue` picker.
     *
     * `recentSessions` is populated only when Bun provides `bun:sqlite` and OpenCode's database exists.
     * On Node-only runs, `recentSessions` is empty and diagnostics use the tmp-directory historian listing.
     */
    recentSessions: RecentSessionSummary[];
    /**
     * `historianDumps` groups historian dumps by project directory.
     * `legacyDumps` contains dumps from the harness-scoped tmp directory.
     */
    historianDumps: HistorianDumpsReport;
}

/**
 * Each bucket groups historian dumps for one project directory represented in `recentSessions`.
 *
 * A bucket exists only for a project directory containing at least one dump under `<directory>/.eidnara/context/historian/`.
 * Sessions that share a project directory use the same bucket.
 * Empty buckets are omitted.
 */
export interface ProjectHistorianBucket {
    /** `directory` identifies the project represented by this bucket. */
    directory: string;
    /** `mostRecentSession` supplies the picker label for this project. */
    primarySessionId: string;
    /** `sessionIds` contains every recent session ID associated with this directory. */
    sessionIds: string[];
    /** `dumpCount` is the total number of dumps in this directory. */
    count: number;
    /** recent contains at most five newest dumps with parsed metadata. */
    recent: HistorianDumpSummary[];
}

export interface HistorianDumpsReport {
    /** byProject orders project buckets by latest activity. */
    byProject: ProjectHistorianBucket[];
    /**
     * `legacyDumps` includes dumps under `${tmpdir}/opencode/eidnara/historian/`.
     */
    legacyDumps: {
        dir: string;
        count: number;
        recent: HistorianDumpSummary[];
    };
}

export interface RecentSessionSummary {
    sessionId: string;
    /** title contains the OpenCode session title and is empty for fresh sessions. */
    title: string;
    /* */
    directory: string;
    /** lastActiveAt contains `session.time_updated` as an ISO timestamp. */
    lastActiveAt: string;
}

function getSelfVersion(): string {
    // createRequire resolves paths relative to this module.
    // The source module is `src/cli/diagnostics.ts`; the bundled module is `dist/cli.js`.
    const require = createRequire(import.meta.url);
    for (const relPath of ["../../package.json", "../package.json"]) {
        try {
            const pkg = require(relPath) as { version?: unknown };
            if (typeof pkg.version === "string" && pkg.version.length > 0) {
                return pkg.version;
            }
        } catch {}
    }
    return "unknown";
}

// ── Sanitization ─────────────────────────────────────────────────────

// Paths can carry secret material through environment overrides (`EIDNARA_LOG_PATH=/tmp/token=abc/...`).
function sanitizeString(value: string): string {
    return sanitizeDiagnosticText(value);
}

function sanitizeValue(value: unknown): unknown {
    return sanitizeConfigValue(value);
}

/**
 * Version-probe output is external process text: a wrapper can print warnings on extra lines,
 * which would inject Markdown lines and break table rows.
 */
export function describeProbeText(text: string): string {
    return sanitizeDiagnosticText(text)
        .replace(/\s*[\r\n]+\s*/g, " ")
        .trim();
}

function readUserOpenCodeConfigs(configDir: string): {
    values: Array<Record<string, unknown> | null>;
    error?: string;
} {
    if (!configDir) return { values: [] };
    const parsed = ["opencode.json", "opencode.jsonc"].map((name) =>
        readConfig(join(configDir, name)),
    );
    const errors = parsed.flatMap((entry) => (entry.error ? [entry.error] : []));
    return {
        values: parsed.map((entry) => entry.value),
        ...(errors.length > 0 ? { error: errors.join("; ") } : {}),
    };
}

/** A FIFO or directory at a config path is a parse error rather than a blocking read. */
function readConfig(path: string): { value: Record<string, unknown> | null; error?: string } {
    let raw: string;
    try {
        raw = readRegularFileSync(path);
    } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") return { value: null };
        return { value: null, error: error instanceof Error ? error.message : String(error) };
    }
    try {
        const value = parseJsonc(raw) as Record<string, unknown>;
        return { value };
    } catch (error) {
        return { value: null, error: error instanceof Error ? error.message : String(error) };
    }
}

function readEidnaraConfigTier(basePath: string): EidnaraConfigTier {
    const detected = detectConfigFile(basePath);
    const parsed = readConfig(detected.path);
    return {
        path: detected.path,
        exists: detected.format !== "none",
        ...(parsed.error ? { parseError: parsed.error } : {}),
        flags: (sanitizeValue(parsed.value ?? {}) as Record<string, unknown>) ?? {},
    };
}

function configHasPluginEntry(config: Record<string, unknown> | null, baseDir: string): boolean {
    const plugins = Array.isArray(config?.plugin) ? config.plugin : [];
    // `ensurePluginEntry` treats a local checkout of this package as registered; diagnostics must agree.
    return plugins.some(
        (entry) =>
            matchesPluginEntry(entry, OPENCODE_PLUGIN_NAME) || isDevPathPluginEntry(entry, baseDir),
    );
}

/**
 * The host merges every project file that exists, `.json` and `.jsonc` alike, so each one is
 * inspected; under `OPENCODE_DISABLE_PROJECT_CONFIG` it loads none of them.
 */
function readProjectOpenCodeConfigs(cwd: string): ProjectOpenCodeConfigReport {
    const report: ProjectOpenCodeConfigReport = { paths: [], hasPlugin: false, parseErrors: [] };
    if (projectConfigDisabled()) return report;
    for (const path of projectOpenCodeConfigPaths(cwd)) {
        if (!existsSync(path)) continue;
        report.paths.push(path);
        const parsed = readConfig(path);
        if (parsed.error) report.parseErrors.push(parsed.error);
        if (configHasPluginEntry(parsed.value, cwd)) report.hasPlugin = true;
    }
    return report;
}

/**
 *
 */
export function collectHistorianDumps(
    recentSessions: RecentSessionSummary[],
): DiagnosticReport["historianDumps"] {
    // The query processes sessions in descending time order; the first session for a directory becomes that bucket's primarySessionId.
    const buckets = new Map<string, ProjectHistorianBucket>();
    for (const session of recentSessions) {
        const dir = session.directory;
        if (!dir) continue;
        const existing = buckets.get(dir);
        if (existing) {
            // When multiple sessions use a directory, append the session ID without recomputing that directory's listing.
            if (!existing.sessionIds.includes(session.sessionId)) {
                existing.sessionIds.push(session.sessionId);
            }
            continue;
        }
        const projectHistorianDir = getProjectEidnaraHistorianDir(dir);
        const listing = listDumpsInDir(projectHistorianDir, 5);
        if (listing.count === 0) continue;
        buckets.set(dir, {
            directory: dir,
            primarySessionId: session.sessionId,
            sessionIds: [session.sessionId],
            count: listing.count,
            recent: listing.recent,
        });
    }

    const legacyDir = getEidnaraHistorianDir("opencode");
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
 *
 * The list limits historian-dump lookups to existing OpenCode sessions.
 * The session list groups project directories and powers the `--issue` flow's session picker.
 *
 */
async function collectRecentSessions(): Promise<RecentSessionSummary[]> {
    // `getDataDir` applies the daemon's rules: a relative `XDG_DATA_HOME` is ignored and an
    // absolute `HOME` is required, so a checkout cannot redirect the lookup. Without a data
    // directory or a database there are no sessions to report.
    let candidates: string[];
    try {
        candidates = resolveOpenCodeDatabaseCandidates(getDataDir());
    } catch {
        return [];
    }

    if (typeof (globalThis as { Bun?: unknown }).Bun === "undefined") {
        return [];
    }

    type DatabaseCtor = new (
        path: string,
        opts?: { readonly?: boolean },
    ) => {
        prepare: (sql: string) => { all: () => unknown[] };
        close: () => void;
    };

    let DatabaseClass: DatabaseCtor;
    try {
        const mod = (await new Function("p", "return import(p)")("bun:sqlite")) as {
            Database: DatabaseCtor;
        };
        DatabaseClass = mod.Database;
    } catch {
        return [];
    }

    // A candidate that is not an OpenCode session database (a stray `opencode-backup.db`, a
    // corrupt file) fails the query; the next-ranked candidate is tried instead.
    for (const candidate of candidates) {
        const rows = querySessions(DatabaseClass, candidate);
        if (rows !== null) return rows;
    }
    return [];
}

function querySessions(
    DatabaseClass: new (
        path: string,
        opts?: { readonly?: boolean },
    ) => { prepare: (sql: string) => { all: () => unknown[] }; close: () => void },
    opencodeDbPath: string,
): RecentSessionSummary[] | null {
    let db: { prepare: (sql: string) => { all: () => unknown[] }; close: () => void } | null = null;
    try {
        db = new DatabaseClass(opencodeDbPath, { readonly: true });
        const rows = db
            .prepare(
                // `session.time_updated` orders sessions as the recency proxy.
                // child's directory.
                "SELECT id, directory, title, time_updated FROM session " +
                    "WHERE time_archived IS NULL AND parent_id IS NULL " +
                    "ORDER BY time_updated DESC LIMIT 5",
            )
            .all() as Array<{
            id: unknown;
            directory: unknown;
            title: unknown;
            time_updated: unknown;
        }>;
        return rows.flatMap((row) => {
            const sessionId = typeof row.id === "string" ? row.id : null;
            const directory = typeof row.directory === "string" ? row.directory : null;
            if (!sessionId || !directory) return [];
            const title = typeof row.title === "string" ? row.title : "";
            const lastActiveAt =
                typeof row.time_updated === "number"
                    ? new Date(row.time_updated).toISOString()
                    : "";
            return [{ sessionId, title, directory, lastActiveAt }];
        });
    } catch {
        return null;
    } finally {
        try {
            db?.close();
        } catch {}
    }
}

/**
 * With `HOME` and `XDG_CONFIG_HOME` unset for a UID without a passwd entry, every user-level path
 * resolution throws; the report then carries empty user-level paths and the error text.
 */
function resolveUserLevelPaths(): { configPaths: ConfigPaths; error?: string } {
    try {
        return { configPaths: detectConfigPaths() };
    } catch (error) {
        return {
            configPaths: {
                configDir: "",
                opencodeConfig: "",
                opencodeConfigFormat: "none",
                eidnaraConfig: "",
                omoConfig: null,
                tuiConfig: "",
                tuiConfigFormat: "none",
            },
            error: error instanceof Error ? error.message : String(error),
        };
    }
}

function readUserEidnaraConfigTier(): EidnaraConfigTier {
    try {
        const basePath = eidnaraUserConfigBasePath();
        if (basePath === undefined) return { path: "", exists: false, flags: {} };
        return readEidnaraConfigTier(basePath);
    } catch {
        return { path: "", exists: false, flags: {} };
    }
}

export async function collectDiagnostics(cwd = process.cwd()): Promise<DiagnosticReport> {
    const pluginVersion = getSelfVersion();
    const userLevel = resolveUserLevelPaths();
    const configPaths = userLevel.configPaths;
    // The host merges `opencode.json` and `opencode.jsonc` from the user config directory, so a
    // registration in either counts; parse errors from both are reported.
    const opencodeConfig = readUserOpenCodeConfigs(configPaths.configDir);
    const tuiConfig = configPaths.tuiConfig ? readConfig(configPaths.tuiConfig) : { value: null };
    const eidnaraConfig = readUserEidnaraConfigTier();
    const projectConfig = readEidnaraConfigTier(eidnaraProjectConfigBasePath(cwd));

    const logPath = getEidnaraLogPath("opencode");
    // The log can be rotated or removed between the existence check and the stat; a vanished log is reported as absent.
    let logFileSize: number | null = null;
    try {
        logFileSize = statSync(logPath).size;
    } catch {
        logFileSize = null;
    }

    let compactionEnabled = false;
    try {
        compactionEnabled = isCompactionEnabled(loadPluginConfig(cwd));
    } catch (error) {
        console.warn(
            `[eidnara] Could not load Eidnara config to resolve compaction mode; ` +
                `preserving existing native compaction fields. ` +
                `(${error instanceof Error ? error.message : String(error)})`,
        );
    }
    // `detectConflicts` reads the `.omo` config through an unguarded home lookup; a host without a
    // home directory must still get the rest of the report.
    let conflictResult: Pick<ConflictResult, "hasConflict" | "reasons" | "nativeCompaction">;
    let conflictsError: string | undefined;
    try {
        conflictResult = detectConflicts(cwd, { compactionEnabled });
    } catch (error) {
        conflictsError = error instanceof Error ? error.message : String(error);
        conflictResult = {
            hasConflict: false,
            reasons: [],
            nativeCompaction: { auto: false, prune: false },
        };
    }
    const recentSessions = await collectRecentSessions();
    const opencodeInstallations = describeOpenCodeInstallations(detectOpenCodeInstallations());
    const activeInstallation = opencodeInstallations[0];
    let openCodeInstallKind: "cli" | "desktop" | "none" = "none";
    if (activeInstallation) openCodeInstallKind = activeInstallation.kind;

    return {
        timestamp: new Date().toISOString(),
        platform: process.platform,
        arch: process.arch,
        nodeVersion: process.version,
        pluginVersion,
        opencodeInstalled: openCodeInstallKind !== "none",
        opencodeInstallKind: openCodeInstallKind,
        opencodeVersion:
            activeInstallation?.kind === "cli" && activeInstallation.version !== "unknown"
                ? activeInstallation.version
                : null,
        opencodeInstallations,
        configPaths,
        ...(userLevel.error ? { configPathsError: userLevel.error } : {}),
        projectDirectory: cwd,
        opencodeConfigHasPlugin: opencodeConfig.values.some((value) =>
            configHasPluginEntry(value, cwd),
        ),
        ...(opencodeConfig.error ? { opencodeConfigParseError: opencodeConfig.error } : {}),
        tuiConfigHasPlugin: configHasPluginEntry(tuiConfig.value, cwd),
        ...(tuiConfig.error ? { tuiConfigParseError: tuiConfig.error } : {}),
        projectOpencodeConfig: readProjectOpenCodeConfigs(cwd),
        eidnaraConfig,
        projectConfig,
        conflicts: {
            hasConflict: conflictResult.hasConflict,
            reasons: conflictResult.reasons,
            compactionEnabled,
            nativeCompaction: conflictResult.nativeCompaction,
            ...(conflictsError ? { detectionError: conflictsError } : {}),
        },
        logFile: {
            path: logPath,
            exists: logFileSize !== null,
            sizeKb: Math.round((logFileSize ?? 0) / 1024),
        },
        recentSessions,
        historianDumps: collectHistorianDumps(recentSessions),
    };
}

export function renderDiagnosticsMarkdown(report: DiagnosticReport): string {
    const configPaths = {
        configDir: sanitizeString(report.configPaths.configDir),
        opencodeConfig: sanitizeString(report.configPaths.opencodeConfig),
        opencodeConfigFormat: report.configPaths.opencodeConfigFormat,
        eidnaraConfig: report.configPaths.eidnaraConfig
            ? sanitizeString(report.configPaths.eidnaraConfig)
            : null,
        tuiConfig: sanitizeString(report.configPaths.tuiConfig),
        tuiConfigFormat: report.configPaths.tuiConfigFormat,
        omoConfig: report.configPaths.omoConfig
            ? sanitizeString(report.configPaths.omoConfig)
            : null,
    };

    const openCodeInstallations = report.opencodeInstallations ?? [];
    const openCodeInstallationTable =
        openCodeInstallations.length > 1
            ? [
                  "",
                  "### OpenCode installations",
                  "| Marker | Path | Version | Source |",
                  "| --- | --- | --- | --- |",
                  ...openCodeInstallations.map(
                      (installation) =>
                          `| ${installation.active ? "[active]" : ""} | \`${sanitizeString(installation.path)}\` | ${describeProbeText(installation.version)} | ${installation.source} |`,
                  ),
              ]
            : [];

    // `parseError` is a raw filesystem or parser message and can name the full local path.
    const sanitizeDumps = (dumps: HistorianDumpSummary[]) =>
        dumps.map((dump) => ({
            ...dump,
            name: sanitizeString(dump.name),
            ...(dump.parseError ? { parseError: sanitizeDiagnosticText(dump.parseError) } : {}),
        }));

    const historianDumps = {
        byProject: report.historianDumps.byProject.map((bucket) => ({
            directory: sanitizeString(bucket.directory),
            primarySessionId: bucket.primarySessionId,
            sessionIds: bucket.sessionIds,
            count: bucket.count,
            recent: sanitizeDumps(bucket.recent),
        })),
        legacyDumps: {
            dir: sanitizeString(report.historianDumps.legacyDumps.dir),
            count: report.historianDumps.legacyDumps.count,
            recent: sanitizeDumps(report.historianDumps.legacyDumps.recent),
        },
    };

    // Titles are user prose (often the first prompt); the picker keeps them, the shareable report does not.
    const recentSessions = report.recentSessions.map((session) => ({
        sessionId: session.sessionId,
        title: session.title ? describeProseLength(session.title) : "",
        directory: sanitizeString(session.directory),
        lastActiveAt: session.lastActiveAt,
    }));

    const describeConfigTier = (tier: EidnaraConfigTier) =>
        `\`${sanitizeString(tier.path)}\`${tier.exists ? "" : " (missing)"}`;
    const describeParseError = (error: string | undefined) =>
        error ? sanitizeDiagnosticText(error) : "none";

    const configPathsJson = JSON.stringify(configPaths, null, 2);
    const userFlagsJson = JSON.stringify(sanitizeConfigValue(report.eidnaraConfig.flags), null, 2);
    const projectFlagsJson = JSON.stringify(
        sanitizeConfigValue(report.projectConfig.flags),
        null,
        2,
    );
    const recentSessionsJson = JSON.stringify(recentSessions, null, 2);
    const historianDumpsJson = JSON.stringify(historianDumps, null, 2);
    const fence = codeFenceFor(
        configPathsJson,
        userFlagsJson,
        projectFlagsJson,
        recentSessionsJson,
        historianDumpsJson,
    );

    return [
        `- Timestamp: ${report.timestamp}`,
        `- Plugin: v${report.pluginVersion}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        `- OpenCode installed: ${report.opencodeInstalled} [${report.opencodeInstallKind}]${report.opencodeVersion ? ` (${describeProbeText(report.opencodeVersion)})` : ""}`,
        `- Project directory: ${sanitizeString(report.projectDirectory)}`,
        ...(report.configPathsError
            ? [`- User-level paths unavailable: ${sanitizeDiagnosticText(report.configPathsError)}`]
            : []),
        `- Plugin registered in opencode config: ${report.opencodeConfigHasPlugin}`,
        `- opencode config parse error: ${describeParseError(report.opencodeConfigParseError)}`,
        `- Plugin registered in tui config: ${report.tuiConfigHasPlugin}`,
        `- tui config parse error: ${describeParseError(report.tuiConfigParseError)}`,
        `- Plugin registered in project opencode config: ${report.projectOpencodeConfig.hasPlugin}${
            report.projectOpencodeConfig.paths.length === 0
                ? " (no project opencode config)"
                : ` (${report.projectOpencodeConfig.paths.map((p) => `\`${sanitizeString(p)}\``).join(", ")})`
        }`,
        `- project opencode config parse errors: ${
            report.projectOpencodeConfig.parseErrors.length === 0
                ? "none"
                : report.projectOpencodeConfig.parseErrors.map(sanitizeDiagnosticText).join("; ")
        }`,
        `- User config: ${describeConfigTier(report.eidnaraConfig)}`,
        `- User config parse error: ${describeParseError(report.eidnaraConfig.parseError)}`,
        `- Project config: ${describeConfigTier(report.projectConfig)}`,
        `- Project config parse error: ${describeParseError(report.projectConfig.parseError)}`,
        `- Conflicts detected: ${report.conflicts.hasConflict ? report.conflicts.reasons.join("; ") : "none"}${
            report.conflicts.detectionError
                ? ` (detection failed: ${sanitizeDiagnosticText(report.conflicts.detectionError)})`
                : ""
        }`,
        `- Eidnara compaction mode: ${report.conflicts.compactionEnabled ? "on" : "off"}`,
        `- Native compaction: auto=${report.conflicts.nativeCompaction?.auto ?? "unknown"}, prune=${report.conflicts.nativeCompaction?.prune ?? "unknown"}`,
        ...openCodeInstallationTable,
        "",
        "### Config paths",
        `${fence}json`,
        configPathsJson,
        fence,
        "",
        "### User config flags",
        `${fence}jsonc`,
        userFlagsJson,
        fence,
        "",
        "### Project config flags",
        `${fence}jsonc`,
        projectFlagsJson,
        fence,
        "",
        "### Recent sessions",
        recentSessions.length === 0
            ? "_No recent OpenCode sessions found (or OpenCode DB unavailable on this runtime)._"
            : [`${fence}json`, recentSessionsJson, fence].join("\n"),
        "",
        "### Historian dumps",
        "(Metadata only — XML content is not included in this report.)",
        "Dumps are stored per-project under `<project>/.eidnara/context/historian/`.",
        `${fence}json`,
        historianDumpsJson,
        fence,
        "",
        "### Log file",
        `- Path: ${sanitizeString(report.logFile.path)}`,
        `- Exists: ${report.logFile.exists}`,
        `- Size: ${report.logFile.sizeKb} KB`,
    ].join("\n");
}
