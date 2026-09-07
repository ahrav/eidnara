// A static `import { Database } from "bun:sqlite"` crashes the Node CLI before `try/catch` can run.
// Node's ESM loader rejects `bun:` specifiers during resolution.
// If the DB cannot be read, the report still includes all other diagnostics.
import { existsSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { join } from "node:path";
import { loadPluginConfig } from "@eidnara/opencode/config";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import { detectConflicts } from "@eidnara/opencode/shared/conflict-detector";
import { getProjectEidnaraHistorianDir } from "@eidnara/opencode/shared/data-path";
import {
    sanitizeConfigValue,
    sanitizeDiagnosticText,
    sanitizePathString,
} from "@eidnara/opencode/shared/redaction";
import { parse as parseJsonc } from "comment-json";
import { type HistorianDumpSummary, listDumpsInDir } from "./historian-dumps";
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
    opencodeConfigHasPlugin: boolean;
    tuiConfigHasPlugin: boolean;
    eidnaraConfig: {
        exists: boolean;
        parseError?: string;
        flags: Record<string, unknown>;
    };
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

function sanitizeString(value: string): string {
    return sanitizePathString(value);
}

function sanitizeValue(value: unknown): unknown {
    return sanitizeConfigValue(value);
}

function readConfig(path: string): { value: Record<string, unknown> | null; error?: string } {
    if (!existsSync(path)) return { value: null };
    try {
        const raw = readFileSync(path, "utf-8");
        const value = parseJsonc(raw) as Record<string, unknown>;
        return { value };
    } catch (error) {
        return { value: null, error: error instanceof Error ? error.message : String(error) };
    }
}

function configHasPluginEntry(config: Record<string, unknown> | null): boolean {
    const plugins = Array.isArray(config?.plugin) ? config.plugin : [];
    return plugins.some((entry) => {
        if (typeof entry !== "string") return false;
        if (entry === OPENCODE_PLUGIN_NAME) return true;
        if (entry.startsWith(`${OPENCODE_PLUGIN_NAME}@`)) return true;
        return false;
    });
}

/**
 *
 */
function collectHistorianDumps(
    recentSessions: RecentSessionSummary[],
): DiagnosticReport["historianDumps"] {
    // The query processes sessions in descending time order; the first session for a directory becomes that bucket's primarySessionId.
    const buckets = new Map<string, ProjectHistorianBucket>();
    for (const session of recentSessions) {
        const dir = session.directory;
        if (!dir) continue;
        const projectHistorianDir = getProjectEidnaraHistorianDir(dir);
        const listing = listDumpsInDir(projectHistorianDir, 5);
        const existing = buckets.get(dir);
        if (existing) {
            // When multiple sessions use a directory, append the session ID without recomputing that directory's listing.
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
    // Runtime `XDG_DATA_HOME` or `HOME` overrides determine the database path.
    // Node's `homedir()` honors runtime `HOME` overrides; Bun's does not.
    const dataHome =
        process.env.XDG_DATA_HOME || join(process.env.HOME || homedir(), ".local", "share");
    const opencodeDbPath = join(dataHome, "opencode", "opencode.db");
    if (!existsSync(opencodeDbPath)) return [];

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
        return [];
    } finally {
        try {
            db?.close();
        } catch {}
    }
}

export async function collectDiagnostics(): Promise<DiagnosticReport> {
    const pluginVersion = getSelfVersion();
    const configPaths = detectConfigPaths();
    const opencodeConfig = readConfig(configPaths.opencodeConfig);
    const tuiConfig = readConfig(configPaths.tuiConfig);
    const eidnaraConfig = readConfig(configPaths.eidnaraConfig);

    const logPath = getEidnaraLogPath("opencode");
    const logFileSize = existsSync(logPath) ? statSync(logPath).size : 0;

    let compactionEnabled = false;
    try {
        compactionEnabled = isCompactionEnabled(loadPluginConfig(process.cwd()));
    } catch (error) {
        console.warn(
            `[eidnara] Could not load Eidnara config to resolve compaction mode; ` +
                `preserving existing native compaction fields. ` +
                `(${error instanceof Error ? error.message : String(error)})`,
        );
    }
    const conflictResult = detectConflicts(process.cwd(), { compactionEnabled });
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
        opencodeConfigHasPlugin: configHasPluginEntry(opencodeConfig.value),
        tuiConfigHasPlugin: configHasPluginEntry(tuiConfig.value),
        eidnaraConfig: {
            exists: existsSync(configPaths.eidnaraConfig),
            ...(eidnaraConfig.error ? { parseError: eidnaraConfig.error } : {}),
            flags: (sanitizeValue(eidnaraConfig.value ?? {}) as Record<string, unknown>) ?? {},
        },
        conflicts: {
            hasConflict: conflictResult.hasConflict,
            reasons: conflictResult.reasons,
            compactionEnabled,
            nativeCompaction: conflictResult.nativeCompaction,
        },
        logFile: {
            path: logPath,
            exists: existsSync(logPath),
            sizeKb: Math.round(logFileSize / 1024),
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
        eidnaraConfig: sanitizeString(report.configPaths.eidnaraConfig),
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
                          `| ${installation.active ? "[active]" : ""} | \`${sanitizeString(installation.path)}\` | ${installation.version} | ${installation.source} |`,
                  ),
              ]
            : [];

    const historianDumps = {
        byProject: report.historianDumps.byProject.map((bucket) => ({
            directory: sanitizeString(bucket.directory),
            primarySessionId: bucket.primarySessionId,
            sessionIds: bucket.sessionIds,
            count: bucket.count,
            recent: bucket.recent,
        })),
        legacyDumps: {
            dir: sanitizeString(report.historianDumps.legacyDumps.dir),
            count: report.historianDumps.legacyDumps.count,
            recent: report.historianDumps.legacyDumps.recent,
        },
    };

    const recentSessions = report.recentSessions.map((session) => ({
        sessionId: session.sessionId,
        title: sanitizeDiagnosticText(session.title),
        directory: sanitizeString(session.directory),
        lastActiveAt: session.lastActiveAt,
    }));

    return [
        `- Timestamp: ${report.timestamp}`,
        `- Plugin: v${report.pluginVersion}`,
        `- OS: ${report.platform} ${report.arch}`,
        `- Node: ${report.nodeVersion}`,
        `- OpenCode installed: ${report.opencodeInstalled} [${report.opencodeInstallKind}]${report.opencodeVersion ? ` (${report.opencodeVersion})` : ""}`,
        `- Plugin registered in opencode config: ${report.opencodeConfigHasPlugin}`,
        `- Plugin registered in tui config: ${report.tuiConfigHasPlugin}`,
        `- eidnara.jsonc parse error: ${report.eidnaraConfig.parseError ?? "none"}`,
        `- Conflicts detected: ${report.conflicts.hasConflict ? report.conflicts.reasons.join("; ") : "none"}`,
        `- Eidnara compaction mode: ${report.conflicts.compactionEnabled ? "on" : "off"}`,
        `- Native compaction: auto=${report.conflicts.nativeCompaction?.auto ?? "unknown"}, prune=${report.conflicts.nativeCompaction?.prune ?? "unknown"}`,
        ...openCodeInstallationTable,
        "",
        "### Config paths",
        "```json",
        JSON.stringify(configPaths, null, 2),
        "```",
        "",
        "### eidnara.jsonc flags",
        "```jsonc",
        JSON.stringify(sanitizeConfigValue(report.eidnaraConfig.flags), null, 2),
        "```",
        "",
        "### Recent sessions",
        recentSessions.length === 0
            ? "_No recent OpenCode sessions found (or OpenCode DB unavailable on this runtime)._"
            : ["```json", JSON.stringify(recentSessions, null, 2), "```"].join("\n"),
        "",
        "### Historian dumps",
        "(Metadata only — XML content is not included in this report.)",
        "Dumps are stored per-project under `<project>/.eidnara/context/historian/`.",
        "```json",
        JSON.stringify(historianDumps, null, 2),
        "```",
        "",
        "### Log file",
        `- Path: ${sanitizeString(report.logFile.path)}`,
        `- Exists: ${report.logFile.exists}`,
        `- Size: ${report.logFile.sizeKb} KB`,
    ].join("\n");
}
