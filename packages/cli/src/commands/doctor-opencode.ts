import { execSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { loadPluginConfig } from "@eidnara/opencode/config";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import { substituteConfigVariables } from "@eidnara/opencode/config/variable";
import { detectConflicts } from "@eidnara/opencode/shared/conflict-detector";
import { fixConflicts } from "@eidnara/opencode/shared/conflict-fixer";
import { ensureTuiPluginEntry } from "@eidnara/opencode/shared/tui-config";
import { parse } from "comment-json";

import {
    isDevPathPluginEntry,
    isLocalPathPluginEntry,
    matchesPluginEntry,
} from "../adapters/opencode";
import { collectDiagnostics } from "../lib/diagnostics-opencode";
import { bundleIssueReport } from "../lib/logs-opencode";
import { detectOpenCodeInstallations } from "../lib/opencode-detect";
import {
    describeOpenCodeInstallations,
    type OpenCodeInstallationReport,
} from "../lib/opencode-helpers";
import { detectConfigPaths, getEidnaraLogPath } from "../lib/paths";
import { confirm, intro, log, outro, selectOne, spinner, text } from "../lib/prompts";

const PLUGIN_NAME = "@eidnara/opencode";

/**
 * On load failure, the helper returns false so native compaction fields are left untouched.
 */
function resolveCompactionEnabledForDoctor(): boolean {
    try {
        const config = loadPluginConfig(process.cwd());
        return isCompactionEnabled(config);
    } catch (error) {
        console.warn(
            `[eidnara] Could not load Eidnara config to resolve compaction mode; ` +
                `preserving existing native compaction fields. ` +
                `(${error instanceof Error ? error.message : String(error)})`,
        );
        return false;
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

        let sessionFilter: string | null = null;
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
                    title,
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

        const url = `https://github.com/ahrav/eidnara/issues/new?title=${encodeURIComponent(title)}&template=bug_report.yml`;
        log.info(
            `Open this URL and paste the contents of ${bundled.path} into the Diagnostics field:`,
        );
        log.info(url);
        openBrowser(url);
        outro("Issue report ready");
        return 0;
    } catch (error) {
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

export async function runDoctor(
    options: { force?: boolean; issue?: boolean } = {},
): Promise<number> {
    if (options.issue) {
        return runIssueFlow();
    }

    intro("Eidnara Doctor");

    let issues = 0;
    let fixed = 0;
    let passCount = 0;
    let warnCount = 0;
    let failCount = 0;
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
        issues++;
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
    }

    log.info(`Eidnara CLI v${getSelfVersion()}`);

    const paths = detectConfigPaths();

    if (paths.opencodeConfigFormat === "none") {
        fail(`No opencode.json found at ${paths.opencodeConfig}`);
    } else {
        pass(`OpenCode config: ${paths.opencodeConfig}`);
    }

    if (existsSync(paths.eidnaraConfig)) {
        pass(`Eidnara config: ${paths.eidnaraConfig}`);
        try {
            const raw = readFileSync(paths.eidnaraConfig, "utf-8");
            const substituted = substituteConfigVariables({
                text: raw,
                configPath: paths.eidnaraConfig,
            }).text;
            parse(substituted);
            pass("eidnara.jsonc parses as valid JSONC");
        } catch (err) {
            fail(`eidnara.jsonc parse failed: ${err instanceof Error ? err.message : String(err)}`);
        }
        try {
            const result = loadPluginConfig(process.cwd());
            const warnings = result.configWarnings ?? [];
            if (warnings.length > 0) {
                warn(
                    `Eidnara config has ${warnings.length} warning(s) — see 'eidnara doctor --issue' for details`,
                );
            } else {
                pass("Eidnara config loads successfully");
            }
        } catch (err) {
            fail(
                `Could not load Eidnara config: ${err instanceof Error ? err.message : String(err)}`,
            );
        }
    } else {
        warn(`No eidnara.jsonc found — using defaults`);
        log.info("  Run 'setup' to create one with model recommendations");
    }

    if (paths.opencodeConfigFormat !== "none") {
        try {
            const raw = readFileSync(paths.opencodeConfig, "utf-8");
            const config = parse(raw) as Record<string, unknown>;
            const rawPlugins: unknown[] = Array.isArray(config?.plugin) ? config.plugin : [];
            const existingIdx = rawPlugins.findIndex(
                (entry) => matchesPluginEntry(entry, PLUGIN_NAME) || isDevPathPluginEntry(entry),
            );
            if (
                rawPlugins.some(
                    (entry) =>
                        isLocalPathPluginEntry(entry) &&
                        String(entry).includes("context") &&
                        !isDevPathPluginEntry(entry),
                )
            ) {
                warn(
                    "An unverifiable local OpenCode plugin path was ignored because its package name is not Eidnara",
                );
            }
            const configName =
                paths.opencodeConfigFormat === "jsonc" ? "opencode.jsonc" : "opencode.json";

            if (existingIdx >= 0) {
                const entry = rawPlugins[existingIdx];
                if (isDevPathPluginEntry(entry)) {
                    pass(
                        `Plugin registered in ${configName} (dev path: ${pluginEntryName(entry)})`,
                    );
                } else {
                    pass(`Plugin registered in ${configName} (${pluginEntryName(entry)})`);
                }
            } else {
                fail(`Plugin ${PLUGIN_NAME} is not registered in ${configName}`);
                log.info("  Run 'setup' to register the plugin");
            }
        } catch {
            warn("Could not parse opencode config to verify plugin entry");
        }
    }

    const cwd = process.cwd();
    const compactionEnabled = resolveCompactionEnabledForDoctor();
    const conflictResult = detectConflicts(cwd, { compactionEnabled });

    // Doctor uses the file-based compaction check because it has no OpenCode server handle.
    log.info(
        "Compaction check: file-based; the running server's resolved config may differ — `opencode debug config` is authoritative",
    );

    if (conflictResult.hasConflict) {
        for (const reason of conflictResult.reasons) {
            fail(`Conflict: ${reason}`);
        }
        if (options.force) {
            const actions = fixConflicts(cwd, conflictResult.conflicts, { compactionEnabled });
            for (const action of actions) {
                pass(`Fixed: ${action}`);
                fixed++;
            }
            if (actions.length > 0) {
                warn("Restart OpenCode for conflict fixes to take effect");
            }
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

    const tuiAdded = ensureTuiPluginEntry();
    if (tuiAdded) {
        pass("Added TUI sidebar plugin to tui.json");
        warn("Restart OpenCode to see the sidebar");
        fixed++;
    } else if (existsSync(paths.tuiConfig)) {
        try {
            const tuiRaw = readFileSync(paths.tuiConfig, "utf-8");
            const tuiConfig = parse(tuiRaw) as Record<string, unknown>;
            const tuiRawPlugins: unknown[] = Array.isArray(tuiConfig?.plugin)
                ? tuiConfig.plugin
                : [];
            const tuiIdx = tuiRawPlugins.findIndex(
                (entry) => matchesPluginEntry(entry, PLUGIN_NAME) || isDevPathPluginEntry(entry),
            );
            if (
                tuiRawPlugins.some(
                    (entry) =>
                        isLocalPathPluginEntry(entry) &&
                        String(entry).includes("context") &&
                        !isDevPathPluginEntry(entry),
                )
            ) {
                warn(
                    "An unverifiable local TUI plugin path was ignored because its package name is not Eidnara",
                );
            }
            if (tuiIdx >= 0) {
                const tuiEntry = tuiRawPlugins[tuiIdx];
                if (isDevPathPluginEntry(tuiEntry)) {
                    pass(`TUI sidebar plugin configured (dev path: ${pluginEntryName(tuiEntry)})`);
                } else {
                    pass("TUI sidebar plugin configured");
                }
            } else {
                fail("TUI sidebar plugin is missing after the repair attempt");
            }
        } catch (error) {
            fail(
                `Could not verify TUI sidebar config: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    } else {
        fail("Could not create or verify the TUI sidebar config");
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
    if (issues === 0 && fixed === 0) {
        outro("Everything looks good! ✨");
    } else if (issues > 0 && fixed > 0) {
        outro(`Found ${issues} issue(s), fixed ${fixed}. Restart OpenCode to apply.`);
    } else if (fixed > 0) {
        outro(`Fixed ${fixed} issue(s). Restart OpenCode to apply.`);
    } else {
        outro(`Found ${issues} issue(s) that need manual attention.`);
        return 1;
    }

    return 0;
}
