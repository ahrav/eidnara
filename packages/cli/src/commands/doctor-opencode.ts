import { execSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { basename } from "node:path";
import { loadPluginConfig } from "@eidnara/opencode/config";
import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
} from "@eidnara/opencode/config/config-paths";
import { substituteConfigVariables } from "@eidnara/opencode/config/variable";
import { detectConflicts } from "@eidnara/opencode/shared/conflict-detector";
import { fixConflicts } from "@eidnara/opencode/shared/conflict-fixer";
import { detectConfigFile } from "@eidnara/opencode/shared/jsonc-parser";
import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import { parse } from "comment-json";

import {
    isDevPathPluginEntry,
    isLocalPathPluginEntry,
    matchesPluginEntry,
} from "../adapters/opencode";
import { collectDiagnostics } from "../lib/diagnostics-opencode";
import { compactionEnabledFor } from "../lib/eidnara-modes";
import { EXCLUDE_SESSION_RECORDS } from "../lib/log-records";
import { bundleIssueReport } from "../lib/logs-opencode";
import { detectOpenCodeInstallations } from "../lib/opencode-detect";
import {
    describeOpenCodeInstallations,
    type OpenCodeInstallationReport,
} from "../lib/opencode-helpers";
import { detectConfigPaths, getEidnaraLogPath } from "../lib/paths";
import {
    confirm,
    intro,
    isPromptCancelledError,
    log,
    outro,
    selectOne,
    spinner,
    text,
} from "../lib/prompts";
import { compareVersionStrings } from "../lib/version";
import { OPENCODE_MINIMUM_VERSION } from "./setup-opencode";

const PLUGIN_NAME = "@eidnara/opencode";

/**
 * On load failure, the helper returns false so native compaction fields are left untouched.
 */
interface DoctorEidnaraModes {
    enabled: boolean;
    compactionEnabled: boolean;
}

function resolveEidnaraModesForDoctor(cwd: string): DoctorEidnaraModes {
    try {
        const config = loadPluginConfig(cwd);
        return {
            enabled: config.enabled !== false,
            compactionEnabled: compactionEnabledFor(config),
        };
    } catch (error) {
        console.warn(
            `[eidnara] Could not load Eidnara config to resolve compaction mode; ` +
                `preserving existing native compaction fields. ` +
                `(${error instanceof Error ? error.message : String(error)})`,
        );
        return { enabled: true, compactionEnabled: false };
    }
}

function getSelfVersion(): string {
    const req = createRequire(import.meta.url);
    for (const relPath of ["../../package.json", "../package.json"]) {
        try {
            const pkg = req(relPath) as { version?: unknown };
            if (typeof pkg.version === "string" && pkg.version.length > 0) return pkg.version;
        } catch {
            // try next
        }
    }
    return "0.0.0";
}

function isGhInstalled(): boolean {
    try {
        execSync("gh --version", { stdio: "pipe" });
        return true;
    } catch {
        return false;
    }
}

function openBrowser(url: string): void {
    try {
        if (process.platform === "darwin") {
            const child = spawnSync("open", [url], { stdio: "ignore" });
            if (child.status === 0) return;
        } else if (process.platform === "linux") {
            const child = spawnSync("xdg-open", [url], { stdio: "ignore" });
            if (child.status === 0) return;
        } else if (process.platform === "win32") {
            const child = spawnSync("cmd", ["/c", "start", "", url], { stdio: "ignore" });
            if (child.status === 0) return;
        }
    } catch {
        // Best-effort only.
    }
}

async function runIssueFlow(): Promise<number> {
    intro("Eidnara Issue Report");

    const title = await text("Issue title", {
        placeholder: "Short summary of the problem",
        validate: (value) => (value.trim() ? undefined : "Title is required"),
    });
    const description = await text("Issue description", {
        placeholder: "Describe what happened, what you expected, and repro steps",
        validate: (value) => (value.trim() ? undefined : "Description is required"),
    });

    const s = spinner();
    s.start("Collecting diagnostics");

    try {
        const report = await collectDiagnostics();
        s.stop("Diagnostics collected");

        // A lone discovered session still filters: the append-only log can hold older sessions' records.
        let sessionFilter: string | null = report.recentSessions[0]?.sessionId ?? null;
        if (report.recentSessions.length === 0 && report.sessionDiscovery === "unavailable") {
            // Discovery failed rather than found nothing, so cross-session records
            // are excluded unless the user opts in explicitly.
            const includeAll = await confirm(
                "The OpenCode session database could not be read, so log records cannot be attributed to this session. Include records from every session in the report?",
                false,
            );
            if (!includeAll) sessionFilter = EXCLUDE_SESSION_RECORDS;
        }
        if (report.recentSessions.length > 1) {
            const choice = await selectOne(
                "Which session is this issue about? (filters log lines from other sessions)",
                [
                    ...report.recentSessions.map((session, index) => {
                        const displayTitle = session.title.trim() || "(no title)";
                        const truncatedTitle =
                            displayTitle.length > 50
                                ? `${displayTitle.slice(0, 47)}...`
                                : displayTitle;
                        return {
                            label: `${truncatedTitle} — ${session.sessionId}${index === 0 ? " (most recent)" : ""}`,
                            value: session.sessionId,
                        };
                    }),
                    {
                        label: "All sessions (no filtering)",
                        value: "__all__",
                    },
                ],
            );
            sessionFilter = choice === "__all__" ? null : choice;
        }

        s.start("Bundling issue report");
        const bundled = await bundleIssueReport(report, description, title, sessionFilter);
        // The bundle already sanitizes its copy; the same text goes to `gh` and the browser URL.
        const publicTitle = sanitizeDiagnosticText(title);
        s.stop(`Report written to ${bundled.path}`);

        const shouldSubmit = await confirm("Submit this issue on GitHub now?", true);
        if (shouldSubmit && isGhInstalled()) {
            const result = spawnSync(
                "gh",
                [
                    "issue",
                    "create",
                    "-R",
                    "ahrav/eidnara",
                    "--title",
                    publicTitle,
                    "--body-file",
                    bundled.path,
                ],
                { encoding: "utf-8", stdio: ["ignore", "pipe", "pipe"] },
            );

            if (result.status === 0) {
                log.success(result.stdout.trim());
                outro("Issue submitted — thanks for the report!");
                return 0;
            }

            log.warn(result.stderr.trim() || "gh issue create failed");
        } else if (shouldSubmit && !isGhInstalled()) {
            log.warn("gh CLI not found — falling back to browser");
        }

        const url = `https://github.com/ahrav/eidnara/issues/new?title=${encodeURIComponent(publicTitle)}&template=bug_report.yml`;
        log.info(
            `Open this URL and paste the contents of ${bundled.path} into the Diagnostics field:`,
        );
        log.info(url);
        // A declined submission leaves the report on disk without launching anything.
        if (shouldSubmit) {
            openBrowser(url);
        }
        outro("Issue report ready");
        return 0;
    } catch (error) {
        if (isPromptCancelledError(error)) throw error;
        s.stop("Diagnostic collection failed");
        log.error(error instanceof Error ? error.message : String(error));
        outro("Issue report failed");
        return 1;
    }
}

function logOpenCodeInstallationTable(installations: OpenCodeInstallationReport[]): void {
    log.info("OpenCode installations:");
    log.info("  marker   | path | version | source");
    for (const installation of installations) {
        log.info(
            `  ${installation.active ? "[active]" : "        "} | ${installation.path} | ${installation.version} | ${installation.source}`,
        );
    }
}

function pluginEntryName(entry: unknown): string {
    if (typeof entry === "string") return entry;
    if (Array.isArray(entry) && typeof entry[0] === "string") return entry[0];
    return "";
}

function isUnverifiableLocalPluginEntry(entry: unknown): boolean {
    return (
        isLocalPathPluginEntry(entry) &&
        String(entry).includes("context") &&
        !isDevPathPluginEntry(entry)
    );
}

export async function runDoctor(
    options: { force?: boolean; issue?: boolean; cwd?: string } = {},
): Promise<number> {
    if (options.issue) {
        return runIssueFlow();
    }

    intro("Eidnara Doctor");

    const cwd = options.cwd ?? process.cwd();
    let fixed = 0;
    let passCount = 0;
    let warnCount = 0;
    let failCount = 0;
    // Failures that a `--force` repair verifiably cleared; the exit code excludes them.
    let repairedCount = 0;
    const pass = (msg: string) => {
        log.success(msg);
        passCount++;
    };
    const warn = (msg: string) => {
        log.warn(msg);
        warnCount++;
    };
    const fail = (msg: string) => {
        log.error(msg);
        failCount++;
    };

    // The doctor only reports plugin entries; `setup` owns every write to these files.
    const reportPluginEntry = (configPath: string, configName: string, what: string): boolean => {
        let config: Record<string, unknown>;
        try {
            config = parse(readFileSync(configPath, "utf-8")) as Record<string, unknown>;
        } catch (error) {
            fail(
                `Could not parse ${configName} to verify the ${what} entry: ${error instanceof Error ? error.message : String(error)}`,
            );
            return false;
        }
        const rawPlugins: unknown[] = Array.isArray(config?.plugin) ? config.plugin : [];
        if (rawPlugins.some(isUnverifiableLocalPluginEntry)) {
            warn(
                `An unverifiable local ${what} path in ${configName} was ignored because its package name is not Eidnara`,
            );
        }
        const entry = rawPlugins.find(
            (candidate) =>
                matchesPluginEntry(candidate, PLUGIN_NAME) || isDevPathPluginEntry(candidate),
        );
        if (entry === undefined) {
            fail(`${what} ${PLUGIN_NAME} is not registered in ${configName}`);
            log.info(`  Run 'setup' to register the ${what}`);
            return false;
        }
        pass(
            isDevPathPluginEntry(entry)
                ? `${what} registered in ${configName} (dev path: ${pluginEntryName(entry)})`
                : `${what} registered in ${configName} (${pluginEntryName(entry)})`,
        );
        return true;
    };

    const installationReports = describeOpenCodeInstallations(detectOpenCodeInstallations());
    const activeInstallation = installationReports[0];
    if (!activeInstallation) {
        fail("OpenCode is not installed or not in PATH");
        log.info("Doctor checked ~/.opencode/bin/opencode and each entry in $PATH.");
        log.info(
            "If `which opencode` succeeds outside doctor, your wrapper or shim may not be readable by Node — please share that wrapper in the issue.",
        );
        outro("Doctor failed — install OpenCode first");
        return 1;
    }
    if (installationReports.length > 1) {
        logOpenCodeInstallationTable(installationReports);
    }
    if (activeInstallation.kind === "desktop") {
        pass(
            installationReports.length > 1
                ? "OpenCode Desktop selected for plugin checks (CLI not installed)"
                : "OpenCode Desktop detected (CLI not installed)",
        );
    } else if (activeInstallation.version === "unknown") {
        fail(`OpenCode CLI was found at ${activeInstallation.path} but could not be executed`);
    } else {
        pass(
            installationReports.length > 1
                ? `OpenCode ${activeInstallation.version} installed (active install marked above)`
                : `OpenCode ${activeInstallation.version} installed`,
        );
        if (compareVersionStrings(activeInstallation.version, OPENCODE_MINIMUM_VERSION) < 0) {
            fail(
                `OpenCode ${activeInstallation.version} is older than the required ${OPENCODE_MINIMUM_VERSION}; the plugin may fail to load. Upgrade OpenCode.`,
            );
        } else {
            pass(`OpenCode version meets minimum ${OPENCODE_MINIMUM_VERSION} requirement`);
        }
    }

    log.info(`Eidnara CLI v${getSelfVersion()}`);

    const paths = detectConfigPaths();

    if (paths.opencodeConfigFormat === "none") {
        fail(`No opencode.json found at ${paths.opencodeConfig}`);
    } else {
        pass(`OpenCode config: ${paths.opencodeConfig}`);
    }

    // Both loader tiers are checked; a project-only config is a supported layout.
    const eidnaraConfigTiers = (
        [
            { label: "user", base: eidnaraUserConfigBasePath(), isProjectConfig: false },
            { label: "project", base: eidnaraProjectConfigBasePath(cwd), isProjectConfig: true },
        ] as const
    ).flatMap((tier) => {
        const detected = detectConfigFile(tier.base);
        return detected.format === "none"
            ? []
            : [{ label: tier.label, path: detected.path, isProjectConfig: tier.isProjectConfig }];
    });

    if (eidnaraConfigTiers.length === 0) {
        warn(`No eidnara.jsonc found — using defaults`);
        log.info("  Run 'setup' to create one with model recommendations");
    }
    for (const tier of eidnaraConfigTiers) {
        const fileName = basename(tier.path);
        pass(`Eidnara ${tier.label} config: ${tier.path}`);
        try {
            const raw = readFileSync(tier.path, "utf-8");
            const substituted = substituteConfigVariables({
                text: raw,
                configPath: tier.path,
                isProjectConfig: tier.isProjectConfig,
            }).text;
            parse(substituted);
            pass(`Eidnara ${tier.label} ${fileName} parses as valid JSONC`);
        } catch (err) {
            fail(
                `Eidnara ${tier.label} ${fileName} parse failed: ${err instanceof Error ? err.message : String(err)}`,
            );
        }
    }
    if (eidnaraConfigTiers.length > 0) {
        try {
            const result = loadPluginConfig(cwd);
            const warnings = result.configWarnings ?? [];
            if (warnings.length > 0) {
                // The issue report carries no loader warnings, so the doctor is where they surface.
                for (const warning of warnings.slice(0, 4)) warn(warning);
                if (warnings.length > 4) {
                    warn(`... and ${warnings.length - 4} more config warning(s)`);
                }
            } else {
                pass("Eidnara config loads successfully");
            }
        } catch (err) {
            fail(
                `Could not load Eidnara config: ${err instanceof Error ? err.message : String(err)}`,
            );
        }
    }

    let serverPluginRegistered = false;
    if (paths.opencodeConfigFormat !== "none") {
        const configName =
            paths.opencodeConfigFormat === "jsonc" ? "opencode.jsonc" : "opencode.json";
        serverPluginRegistered = reportPluginEntry(paths.opencodeConfig, configName, "Plugin");
    }

    const modes = resolveEidnaraModesForDoctor(cwd);
    const compactionEnabled = modes.compactionEnabled;
    // With `enabled: false` the plugin skips every hook, so nothing here would
    // replace native compaction, DCP, or the OMO hooks; they are left in place.
    const conflictResult = modes.enabled ? detectConflicts(cwd, { compactionEnabled }) : null;

    // Doctor uses the file-based compaction check because it has no OpenCode server handle.
    log.info(
        "Compaction check: file-based; the running server's resolved config may differ — `opencode debug config` is authoritative",
    );

    if (conflictResult === null) {
        pass(
            "Eidnara is disabled (enabled: false); native compaction, DCP, and OMO hooks are left in place",
        );
    } else if (conflictResult.hasConflict) {
        for (const reason of conflictResult.reasons) {
            fail(`Conflict: ${reason}`);
        }
        if (options.force && !serverPluginRegistered) {
            // Disabling native compaction with no registered plugin would leave
            // the installation with no context-window manager at all.
            fail(
                `Leaving conflicts in place: ${PLUGIN_NAME} is not registered in the OpenCode config, so nothing would replace native compaction. Run 'setup' first.`,
            );
        } else if (options.force) {
            try {
                const actions = fixConflicts(cwd, conflictResult.conflicts, { compactionEnabled });
                for (const action of actions) {
                    pass(`Fixed: ${action}`);
                    fixed++;
                }
                if (actions.length > 0) {
                    warn("Restart OpenCode for conflict fixes to take effect");
                }
            } catch (error) {
                // `fixConflicts` can partially repair files before failing; the second
                // `detectConflicts` reports the on-disk state.
                fail(
                    `Conflict repair failed: ${error instanceof Error ? error.message : String(error)}`,
                );
            }
            const remaining = detectConflicts(cwd, { compactionEnabled });
            repairedCount = conflictResult.reasons.filter(
                (reason) => !remaining.reasons.includes(reason),
            ).length;
        } else {
            log.info("  Run 'doctor --force' to repair these conflicts");
        }
    } else {
        // When compaction is off, native `compaction.auto=true` activates native compaction and is not a conflict.
        if (!compactionEnabled) {
            if (conflictResult.nativeCompaction.auto || conflictResult.nativeCompaction.prune) {
                pass(
                    "No conflicts detected (compaction, DCP, OMO hooks) — native compaction active (compaction-off mode)",
                );
            } else {
                warn(
                    "No compaction manager is active: Eidnara compaction is off and OpenCode auto-compaction is disabled",
                );
            }
        } else {
            pass("No conflicts detected (compaction, DCP, OMO hooks)");
        }
    }

    if (paths.tuiConfigFormat === "none") {
        fail(
            `TUI sidebar plugin ${PLUGIN_NAME} is not registered (no tui.json at ${paths.tuiConfig})`,
        );
        log.info("  Run 'setup' to register the TUI sidebar plugin");
    } else {
        const tuiConfigName = paths.tuiConfigFormat === "jsonc" ? "tui.jsonc" : "tui.json";
        reportPluginEntry(paths.tuiConfig, tuiConfigName, "TUI sidebar plugin");
    }

    const logPath = getEidnaraLogPath("opencode");
    if (existsSync(logPath)) {
        const logStat = statSync(logPath);
        const sizeKb = (logStat.size / 1024).toFixed(0);
        log.info(`Log file: ${logPath} (${sizeKb} KB)`);
    } else {
        log.info(`Log file: ${logPath} (not yet created)`);
    }

    // Dumps are grouped by project so users can identify each project's dumps.
    const diagnostics = await collectDiagnostics();
    const dumpBuckets = diagnostics.historianDumps.byProject;
    if (dumpBuckets.length > 0) {
        const totalCount = dumpBuckets.reduce((sum, b) => sum + b.count, 0);
        const sessionCount = dumpBuckets.length;
        warn(`Historian debug dumps: ${totalCount} file(s) across ${sessionCount} project(s)`);
        for (const bucket of dumpBuckets) {
            log.info(`  [${bucket.directory}] ${bucket.count} file(s)`);
            for (const dump of bucket.recent.slice(0, 3)) {
                const age = dump.ageMinutes;
                const ageStr = age < 60 ? `${age}m ago` : `${Math.round(age / 60)}h ago`;
                log.info(`    ${dump.name} (${ageStr})`);
            }
            if (bucket.count > 3) {
                log.info(`    ... and ${bucket.count - 3} more`);
            }
        }
    }
    const legacy = diagnostics.historianDumps.legacyDumps;
    if (legacy.count > 0) {
        log.info(`Legacy historian dumps (pre-v0.18.x): ${legacy.count} file(s) in ${legacy.dir}`);
    }

    if (paths.omoConfig) {
        log.info(`OMO config found: ${paths.omoConfig}`);
    }

    console.log("");
    log.message(`Summary: PASS ${passCount} / WARN ${warnCount} / FAIL ${failCount}`);
    const unresolved = failCount - repairedCount;
    if (unresolved === 0 && fixed === 0) {
        outro("Everything looks good! ✨");
        return 0;
    }
    if (unresolved === 0) {
        outro(`Fixed ${fixed} issue(s). Restart OpenCode to apply.`);
        return 0;
    }
    if (fixed > 0) {
        outro(
            `Fixed ${fixed} issue(s); ${unresolved} issue(s) still need manual attention. Restart OpenCode to apply the fixes.`,
        );
    } else {
        outro(`Found ${unresolved} issue(s) that need manual attention.`);
    }
    return 1;
}
