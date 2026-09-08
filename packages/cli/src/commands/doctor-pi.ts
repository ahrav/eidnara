import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname } from "node:path";
import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
} from "@eidnara/opencode/config/config-paths";
import { EidnaraConfigSchema } from "@eidnara/opencode/config/schema/eidnara";
import { detectConfigFile } from "@eidnara/opencode/shared/jsonc-parser";
import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import { loadPiConfig } from "@eidnara/pi/config";
import { stringify as stringifyJsonc } from "comment-json";

import { writeFileAtomic } from "../lib/atomic-write";
import { collectDiagnostics, sanitizeString } from "../lib/diagnostics-pi";
import { describeHistorianDumps } from "../lib/historian-dumps";
import { readJsoncLenient } from "../lib/jsonc-config";
import { EXCLUDE_SESSION_RECORDS } from "../lib/log-records";
import { readLogTailLines } from "../lib/log-tail";
import { bundleIssueReport } from "../lib/logs-pi";
import { getEidnaraLogPath, getPiAgentDir, getPiUserExtensionsPath } from "../lib/paths";
import {
    detectPiBinary,
    getPiVersion,
    isEidnaraPiPackageEntry,
    PI_MINIMUM_VERSION,
    PI_PACKAGE_SOURCE,
    type PiBinaryInfo,
} from "../lib/pi-helpers";
import { isPromptCancelledError, type PromptIO, promptIO } from "../lib/prompts";
import { standaloneVersion } from "../lib/semver";
import { compareVersionStrings } from "../lib/version";
import { writePiSettingsPackage } from "./setup-pi";

type CheckStatus = "pass" | "warn" | "fail" | "info";

interface CheckResult {
    status: CheckStatus;
    message: string;
}

interface RepairPlan {
    /** False when Pi is missing, reports no usable version, or is below the floor; the package entry then stays untouched. */
    hostSupported: boolean;
    addPackageEntry: boolean;
    writeUserConfig: boolean;
}

interface HealthReport {
    results: CheckResult[];
    repairPlan: RepairPlan;
    pass: number;
    warn: number;
    fail: number;
}

interface DoctorDeps {
    prompts: PromptIO;
    collectDiagnostics: typeof collectDiagnostics;
    detectPiBinary: () => PiBinaryInfo | null;
    getPiVersion: (piPath: string) => string | null;
    selfVersion: () => string;
    now: () => Date;
    execFileSync: typeof execFileSync;
    spawnSync: typeof spawnSync;
}

export interface RunDoctorOptions {
    force?: boolean;
    issue?: boolean;
    cwd?: string;
    prompts?: PromptIO;
    deps?: Partial<DoctorDeps>;
}

const DEFAULT_DEPS: DoctorDeps = {
    prompts: promptIO,
    collectDiagnostics,
    detectPiBinary,
    getPiVersion,
    selfVersion,
    now: () => new Date(),
    execFileSync,
    spawnSync,
};

function depsFrom(options: RunDoctorOptions): DoctorDeps {
    return {
        ...DEFAULT_DEPS,
        prompts: options.prompts ?? DEFAULT_DEPS.prompts,
        ...options.deps,
    };
}

function selfVersion(): string {
    const req = createRequire(import.meta.url);
    for (const relPath of ["../../package.json", "../package.json"]) {
        try {
            const pkg = req(relPath) as { version?: unknown };
            if (typeof pkg.version === "string") return pkg.version;
        } catch {}
    }
    return "unknown";
}

/** Collapses multi-line output such as a stack trace to one quoted, bounded line. */
function describeVersionOutput(output: string): string {
    const MAX_CHARS = 120;
    const flattened = output.replace(/\s+/g, " ").trim();
    return JSON.stringify(
        flattened.length > MAX_CHARS ? `${flattened.slice(0, MAX_CHARS)}…` : flattened,
    );
}

/** The plugin log is append-only and never rotated, so reads of it are bounded. */
const LOG_TAIL_BYTES = 64 * 1024;

function readLastNonEmptyLine(path: string): string | undefined {
    return readLogTailLines(path, LOG_TAIL_BYTES)
        .map((line) => line.trim())
        .filter(Boolean)
        .at(-1);
}

function add(results: CheckResult[], status: CheckStatus, message: string): void {
    results.push({ status, message });
}

function printResult(prompts: PromptIO, result: CheckResult): void {
    const line = `${result.status.toUpperCase()} ${result.message}`;
    if (result.status === "pass") prompts.log.success(line);
    else if (result.status === "info") prompts.log.info(line);
    else if (result.status === "warn") prompts.log.warn(line);
    else console.error(line);
}

function summarize(results: CheckResult[]): Pick<HealthReport, "pass" | "warn" | "fail"> {
    return {
        pass: results.filter((result) => result.status === "pass").length,
        warn: results.filter((result) => result.status === "warn").length,
        fail: results.filter((result) => result.status === "fail").length,
    };
}

function packagesFrom(settings: Record<string, unknown>): unknown[] {
    return Array.isArray(settings.packages) ? settings.packages : [];
}

// Pi resolves a relative `packages[]` path against the settings file's directory.
function isPiEidnaraPackageEntry(entry: unknown): boolean {
    return isEidnaraPiPackageEntry(entry, getPiAgentDir());
}

function describePackageEntry(entry: unknown): string {
    return typeof entry === "string" ? entry : JSON.stringify(entry);
}

async function runHealthChecks(options: {
    cwd: string;
    prompts: PromptIO;
    deps: DoctorDeps;
    quiet?: boolean;
}): Promise<HealthReport> {
    const results: CheckResult[] = [];
    const repairPlan: RepairPlan = {
        hostSupported: true,
        addPackageEntry: false,
        writeUserConfig: false,
    };
    const self = options.deps.selfVersion();

    const pi = options.deps.detectPiBinary();
    if (!pi) {
        repairPlan.hostSupported = false;
        add(results, "fail", "Pi binary not found on PATH or at ~/.pi/bin/pi");
    } else {
        const output = options.deps.getPiVersion(pi.path);
        // `getPiVersion` returns the raw `pi --version` output; only a line
        // that is nothing but a version counts, so a warning that quotes some
        // other tool's version cannot pass as Pi's.
        const version = standaloneVersion(output);
        if (output === null) {
            repairPlan.hostSupported = false;
            add(results, "fail", `Pi CLI was found at ${pi.path} but could not be executed`);
        } else if (version === null) {
            repairPlan.hostSupported = false;
            add(
                results,
                "fail",
                `Pi CLI at ${pi.path} printed unrecognized version output: ${describeVersionOutput(output)}`,
            );
        } else if (compareVersionStrings(version, PI_MINIMUM_VERSION) < 0) {
            repairPlan.hostSupported = false;
            add(results, "pass", `Pi ${version} detected at ${pi.path}`);
            add(
                results,
                "fail",
                `Pi ${version} is older than required ${PI_MINIMUM_VERSION}, the floor of the \`@earendil-works/pi-coding-agent\` range ${PI_PACKAGE_SOURCE} declares; older hosts may not load the extension. Run \`pi update\` (or \`npm install -g @earendil-works/pi-coding-agent@latest\`).`,
            );
        } else {
            add(results, "pass", `Pi ${version} detected at ${pi.path}`);
            add(results, "pass", `Pi version meets minimum ${PI_MINIMUM_VERSION} requirement`);
        }
    }

    if (standaloneVersion(self) !== null) {
        add(results, "info", `Eidnara for Pi CLI v${self}`);
    } else {
        add(results, "info", "Eidnara for Pi CLI version unknown");
    }

    const settingsPath = getPiUserExtensionsPath();
    let packages: unknown[] = [];
    if (!existsSync(settingsPath)) {
        add(results, "fail", `Pi settings not found at ${settingsPath}`);
        repairPlan.addPackageEntry = true;
    } else {
        const parsed = readJsoncLenient(settingsPath);
        if (parsed.parseError) {
            add(
                results,
                "fail",
                `Could not parse Pi settings ${settingsPath}: ${parsed.parseError}`,
            );
        } else {
            packages = packagesFrom(parsed.value);
            add(results, "pass", `Pi settings found at ${settingsPath}`);
            if (packages.some(isPiEidnaraPackageEntry)) {
                add(results, "pass", `${PI_PACKAGE_SOURCE} is registered in packages[]`);
            } else {
                add(results, "fail", `${PI_PACKAGE_SOURCE} is missing from packages[]`);
                repairPlan.addPackageEntry = true;
            }
        }
    }

    // Both `.jsonc` and `.json` are loadable config files, and `.jsonc` wins
    // when both exist, so a default `.jsonc` must not be written next to a `.json`.
    const userConfigBase = eidnaraUserConfigBasePath();
    const projectConfig = detectConfigFile(eidnaraProjectConfigBasePath(options.cwd));
    if (userConfigBase === undefined) {
        // No absolute `HOME` or `XDG_CONFIG_HOME`: there is no user tier to check or create.
        add(
            results,
            "fail",
            "No user Eidnara config path: HOME and XDG_CONFIG_HOME are unset or not absolute",
        );
    }
    for (const [label, detected, required] of [
        ...(userConfigBase === undefined
            ? []
            : [["user", detectConfigFile(userConfigBase), true] as const]),
        ["project", projectConfig, false] as const,
    ]) {
        if (detected.format === "none") {
            if (required) {
                add(results, "warn", `No ${label} eidnara.jsonc found at ${detected.path}`);
                repairPlan.writeUserConfig = true;
            } else {
                add(results, "info", `No project Eidnara config found at ${detected.path}`);
            }
            continue;
        }
        const fileName = basename(detected.path);
        const parsed = readJsoncLenient(detected.path);
        if (parsed.parseError)
            add(results, "fail", `${label} ${fileName} is invalid JSONC: ${parsed.parseError}`);
        else add(results, "pass", `${label} ${fileName} is valid JSONC: ${detected.path}`);
    }

    const loadedConfig = loadPiConfig({ cwd: options.cwd });
    if (loadedConfig.warnings.length > 0) {
        for (const warning of loadedConfig.warnings.slice(0, 4)) add(results, "warn", warning);
        if (loadedConfig.warnings.length > 4) {
            add(
                results,
                "warn",
                `... and ${loadedConfig.warnings.length - 4} more config warning(s)`,
            );
        }
    } else {
        add(results, "pass", "Pi Eidnara config loads successfully");
    }

    const historianModel = loadedConfig.config.historian?.model?.trim() ?? "";
    const historianThinkingLevel = loadedConfig.config.historian?.thinking_level;
    if (historianModel.startsWith("github-copilot/") && !historianThinkingLevel) {
        add(
            results,
            "warn",
            `historian.model "${historianModel}" is a GitHub Copilot reasoning model but ` +
                `historian.thinking_level is not set. GitHub Copilot may apply a bad ` +
                `default reasoning_effort that it then rejects (400 error). ` +
                `Set historian.thinking_level to "medium" (or "off" to disable thinking) ` +
                `in your eidnara.jsonc.`,
        );
    } else if (historianModel.startsWith("github-copilot/") && historianThinkingLevel) {
        add(
            results,
            "pass",
            `historian.model "${historianModel}" has thinking_level "${historianThinkingLevel}" configured`,
        );
    }

    // An npm entry and a local development-path entry for the same plugin load the plugin twice.
    const piEntries = packages.filter(isPiEidnaraPackageEntry).map(describePackageEntry);
    if (piEntries.length > 1) {
        add(
            results,
            "fail",
            `Multiple Eidnara entries in Pi packages[] — this loads the plugin twice: ${piEntries.join(", ")}`,
        );
    } else {
        add(results, "pass", "No conflicting Eidnara entries in Pi packages[]");
    }

    const otherExtensions = packages
        .filter((entry) => !isPiEidnaraPackageEntry(entry))
        .map(describePackageEntry);
    if (otherExtensions.length > 0) {
        add(results, "info", `Other Pi extensions registered: ${otherExtensions.join(", ")}`);
    } else {
        add(results, "info", "No other Pi extensions listed in settings.json");
    }

    const logPath = getEidnaraLogPath("pi");
    if (existsSync(logPath)) {
        // A path that exists but cannot be read (permissions, or a directory
        // named by EIDNARA_LOG_PATH) is a broken logging setup, not a doctor crash.
        try {
            const stat = statSync(logPath);
            const sizeKb = (stat.size / 1024).toFixed(0);
            const lastLine = readLastNonEmptyLine(logPath);
            add(results, "info", `Log file: ${logPath} (${sizeKb} KB)`);
            add(
                results,
                "info",
                `Last plugin log line: ${lastLine ? sanitizeDiagnosticText(lastLine) : "<empty log>"}`,
            );
        } catch (error) {
            add(
                results,
                "fail",
                `Log file ${logPath} exists but could not be read: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    } else {
        add(results, "info", `No plugin log file yet at ${logPath}`);
    }

    const diagnosticsForDumps = await collectDiagnostics(options.cwd);
    for (const line of describeHistorianDumps(diagnosticsForDumps.historianDumps)) {
        add(results, line.status, line.message);
    }

    if (!options.quiet) {
        for (const result of results) printResult(options.prompts, result);
    }

    return { results, repairPlan, ...summarize(results) };
}

function writeDefaultEidnaraConfig(path: string): void {
    mkdirSync(dirname(path), { recursive: true });
    const config = {
        $schema: "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json",
        ...EidnaraConfigSchema.parse({}),
    };
    writeFileAtomic(path, `${stringifyJsonc(config, null, 2)}\n`);
}

interface RepairOutcome {
    fixed: number;
    failed: number;
}

function repair(plan: RepairPlan, prompts: PromptIO): RepairOutcome {
    const outcome: RepairOutcome = { fixed: 0, failed: 0 };
    if (plan.addPackageEntry && !plan.hostSupported) {
        // Registering the extension on an unverified host would make Pi load a
        // package it may not support; setup asks before doing the same.
        outcome.failed += 1;
        console.error(
            `FAIL Leaving Pi packages[] untouched: Pi is missing, reports no usable version, or is older than ${PI_MINIMUM_VERSION}, so ${PI_PACKAGE_SOURCE} may not load. Install or upgrade Pi first.`,
        );
    } else if (plan.addPackageEntry) {
        const settingsPath = getPiUserExtensionsPath();
        try {
            const added = writePiSettingsPackage(settingsPath);
            prompts.log.success(
                added
                    ? `Added ${PI_PACKAGE_SOURCE} to ${settingsPath}`
                    : `${PI_PACKAGE_SOURCE} already present in ${settingsPath}`,
            );
            outcome.fixed += added ? 1 : 0;
        } catch (error) {
            outcome.failed += 1;
            console.error(
                `FAIL Could not update ${settingsPath}: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    }

    if (plan.writeUserConfig) {
        const userConfigBase = eidnaraUserConfigBasePath();
        const detected = userConfigBase === undefined ? null : detectConfigFile(userConfigBase);
        if (detected !== null && detected.format === "none") {
            try {
                writeDefaultEidnaraConfig(detected.path);
                prompts.log.success(`Wrote default Eidnara config to ${detected.path}`);
                outcome.fixed += 1;
            } catch (error) {
                outcome.failed += 1;
                console.error(
                    `FAIL Could not write ${detected.path}: ${error instanceof Error ? error.message : String(error)}`,
                );
            }
        }
    }

    return outcome;
}

function ghAvailableAndAuthed(deps: DoctorDeps): boolean {
    try {
        deps.execFileSync("gh", ["--version"], {
            stdio: ["ignore", "pipe", "ignore"],
        });
        deps.execFileSync("gh", ["auth", "status"], {
            stdio: ["ignore", "pipe", "ignore"],
        });
        return true;
    } catch {
        return false;
    }
}

async function runIssueFlow(options: {
    cwd: string;
    prompts: PromptIO;
    deps: DoctorDeps;
}): Promise<number> {
    options.prompts.intro("Eidnara for Pi Issue Report");
    const title = await options.prompts.text("Issue title", {
        placeholder: "Short summary of the Pi problem",
        validate: (value) => (value.trim() ? undefined : "Title is required"),
    });
    const description = await options.prompts.text("Issue description", {
        placeholder: "Describe what happened, what you expected, and repro steps",
        validate: (value) => (value.trim() ? undefined : "Description is required"),
    });

    const spinner = options.prompts.spinner();
    spinner.start("Collecting sanitized Pi diagnostics");
    try {
        const report = await options.deps.collectDiagnostics(options.cwd);
        spinner.stop("Diagnostics collected");

        // A lone discovered session still filters: the append-only log can hold older sessions' records.
        let sessionFilter: string | null = report.recentSessions[0]?.sessionId ?? null;
        if (report.recentSessions.length === 0) {
            // Without a discovered session, cross-session records are excluded unless the user opts in.
            const includeAll = await options.prompts.confirm(
                report.sessionDiscovery === "unavailable"
                    ? "The Pi sessions directory could not be read, so log records cannot be attributed to this session. Include records from every session in the report?"
                    : "No Pi sessions were found, so log records cannot be attributed to this session. Include records from every session in the report?",
                false,
            );
            if (!includeAll) sessionFilter = EXCLUDE_SESSION_RECORDS;
        }
        if (report.sessionDiscovery === "partial") {
            options.prompts.log.warn(
                "Some Pi session directories could not be read, so the session list may be incomplete.",
            );
        }
        // An incomplete list still gets the picker for a lone session so "All sessions" stays reachable.
        const showPicker =
            report.recentSessions.length > 1 ||
            (report.recentSessions.length === 1 && report.sessionDiscovery === "partial");
        if (showPicker) {
            const choice = await options.prompts.selectOne(
                "Which Pi session is this issue about? (filters log lines from other sessions)",
                [
                    ...report.recentSessions.map((session, index) => ({
                        label: `${session.directory} — ${session.sessionId}${index === 0 ? " (most recent)" : ""}`,
                        value: session.sessionId,
                    })),
                    {
                        label: "All sessions (no filtering)",
                        value: "__all__",
                    },
                ],
            );
            sessionFilter = choice === "__all__" ? null : choice;
        }

        // A selected session from another project gets that project's config,
        // loader warnings, and dumps, not the current directory's.
        const selected = report.recentSessions.find(
            (session) => session.sessionId === sessionFilter,
        );
        let reportForBundle = report;
        if (selected !== undefined && selected.directory !== options.cwd) {
            spinner.start(`Collecting diagnostics for ${selected.directory}`);
            reportForBundle = await options.deps.collectDiagnostics(selected.directory);
            spinner.stop("Diagnostics collected for the selected session");
        }

        spinner.start("Bundling Pi issue report");
        const bundled = await bundleIssueReport(reportForBundle, description, title, {
            cwd: options.cwd,
            now: options.deps.now(),
            sessionFilter,
        });
        spinner.stop(`Report written to ${bundled.path}`);

        if (ghAvailableAndAuthed(options.deps)) {
            const shouldSubmit = await options.prompts.confirm(
                "Submit this issue on GitHub now?",
                false,
            );
            if (shouldSubmit) {
                const result = options.deps.spawnSync(
                    "gh",
                    [
                        "issue",
                        "create",
                        "-R",
                        "ahrav/eidnara",
                        "--title",
                        `[pi] ${sanitizeString(title)}`,
                        "--body-file",
                        bundled.path,
                    ],
                    { encoding: "utf-8", stdio: ["ignore", "pipe", "pipe"] },
                );
                if (result.status === 0) {
                    options.prompts.log.success(String(result.stdout).trim());
                    options.prompts.outro("Issue submitted — thanks for the report!");
                    return 0;
                }
                options.prompts.log.warn(String(result.stderr).trim() || "gh issue create failed");
            }
        } else {
            options.prompts.log.warn(
                "gh CLI is unavailable or not authenticated; printing report for manual issue creation",
            );
        }

        console.log(bundled.bodyMarkdown);
        options.prompts.log.info(
            `Open https://github.com/ahrav/eidnara/issues/new and attach ${bundled.path}`,
        );
        options.prompts.outro("Issue report ready");
        return 0;
    } catch (error) {
        if (isPromptCancelledError(error)) throw error;
        spinner.stop("Diagnostic collection failed");
        console.error(error instanceof Error ? error.message : String(error));
        options.prompts.outro("Issue report failed");
        return 1;
    }
}

export async function runDoctor(options: RunDoctorOptions = {}): Promise<number> {
    const deps = depsFrom(options);
    const prompts = options.prompts ?? deps.prompts;
    const cwd = options.cwd ?? process.cwd();

    if (options.issue) {
        return runIssueFlow({ cwd, prompts, deps });
    }

    prompts.intro("Eidnara for Pi Doctor");
    const first = await runHealthChecks({ cwd, prompts, deps });
    console.log("");
    prompts.log.message(`Summary: PASS ${first.pass} / WARN ${first.warn} / FAIL ${first.fail}`);

    if (options.force) {
        const repaired = repair(first.repairPlan, prompts);
        console.log("");
        prompts.log.message(
            repaired.failed > 0
                ? `Repair attempted; ${repaired.fixed} item(s) changed, ${repaired.failed} item(s) failed. Re-running health checks.`
                : `Repair attempted; ${repaired.fixed} item(s) changed. Re-running health checks.`,
        );
        const second = await runHealthChecks({ cwd, prompts, deps });
        console.log("");
        prompts.log.message(
            `Summary: PASS ${second.pass} / WARN ${second.warn} / FAIL ${second.fail}`,
        );
        if (repaired.failed > 0) {
            prompts.outro("Doctor could not complete the requested repair");
            return 1;
        }
        prompts.outro(
            second.fail > 0 ? "Doctor found failures after repair" : "Doctor repair complete",
        );
        return second.fail > 0 ? 1 : 0;
    }

    prompts.outro(first.fail > 0 ? "Doctor found failures" : "Doctor complete");
    return first.fail > 0 ? 1 : 0;
}
