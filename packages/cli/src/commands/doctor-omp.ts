import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, join, resolve } from "node:path";
import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
    resolveEidnaraProjectConfigPath,
} from "@eidnara/opencode/config/config-paths";
import { EidnaraConfigSchema } from "@eidnara/opencode/config/schema/eidnara";
import { detectConfigFile } from "@eidnara/opencode/shared/jsonc-parser";
import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import { loadPiConfig } from "@eidnara/pi/config";
import { stringify as stringifyJsonc } from "comment-json";
import { OmpAdapter } from "../adapters/omp";
import type { PluginEntryResult } from "../adapters/types";
import { writeFileAtomic } from "../lib/atomic-write";
import { collectPiHistorianDumps, collectPiRecentSessions } from "../lib/diagnostics-pi";
import { projectModeOverrides, readEidnaraModes } from "../lib/eidnara-modes";
import { writeNewFile } from "../lib/fs-utils";
import { describeHistorianDumps } from "../lib/historian-dumps";
import { capBodyToGithubLimit } from "../lib/issue-body";
import { readJsoncLenient } from "../lib/jsonc-config";
import {
    detectOmpBinary,
    getOmpSetting,
    getOmpVersion,
    listOmpPlugins,
    OMP_PLUGIN_PACKAGE,
    type OmpBinaryInfo,
    runOmpCommand,
} from "../lib/omp-helpers";
import {
    getEidnaraLogPath,
    getOmpAgentDir,
    getOmpConfigPath,
    getOmpNonGlobalConfigSources,
    getOmpPackageDir,
    getOmpPluginsLockPath,
    getOmpSessionsRoot,
    getSharedUserConfigPath,
    hasHomeDir,
} from "../lib/paths";
import { type PromptIO, promptIO } from "../lib/prompts";
import { compareVersionStrings } from "../lib/version";

const MIN_OMP_VERSION = "17.1.7";
type Status = "pass" | "warn" | "fail" | "info";
interface CheckResult {
    status: Status;
    message: string;
}
interface RepairPlan {
    installPlugin: boolean;
    disableCompaction: boolean;
    disableMemory: boolean;
    /** The `memory.backend` value a failed repair restores; set with `disableMemory`. */
    priorMemoryBackend: string | null;
    writeUserConfig: boolean;
    /** False when OMP reports no version or one below the tested minimum; every OMP-side repair then stays off. */
    hostSupported: boolean;
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
    detectOmpBinary: () => OmpBinaryInfo | null;
    ensurePluginEntry: () => Promise<PluginEntryResult>;
    getOmpVersion: typeof getOmpVersion;
    getOmpSetting: typeof getOmpSetting;
    listOmpPlugins: typeof listOmpPlugins;
    runOmpCommand: typeof runOmpCommand;
    now: () => Date;
    execFileSync: typeof execFileSync;
    spawnSync: typeof spawnSync;
}

export interface RunOmpDoctorOptions {
    force?: boolean;
    issue?: boolean;
    cwd?: string;
    prompts?: PromptIO;
    deps?: Partial<DoctorDeps>;
}

const DEFAULT_DEPS: DoctorDeps = {
    prompts: promptIO,
    detectOmpBinary,
    ensurePluginEntry: () => new OmpAdapter().ensurePluginEntry(),
    getOmpVersion,
    getOmpSetting,
    listOmpPlugins,
    runOmpCommand,
    now: () => new Date(),
    execFileSync,
    spawnSync,
};

function add(results: CheckResult[], status: Status, message: string): void {
    results.push({ status, message });
}

function printResult(prompts: PromptIO, result: CheckResult): void {
    const line = `${result.status.toUpperCase()} ${result.message}`;
    if (result.status === "pass") prompts.log.success(line);
    else if (result.status === "warn") prompts.log.warn(line);
    else if (result.status === "info") prompts.log.info(line);
    else prompts.log.error(line);
}

function selfVersion(): string {
    const req = createRequire(import.meta.url);
    for (const path of ["../../package.json", "../package.json"]) {
        try {
            const value = req(path) as { version?: unknown };
            if (typeof value.version === "string") return value.version;
        } catch {}
    }
    return "unknown";
}

function pluginDeclaresOmp(path: string | undefined): boolean | null {
    if (!path) return null;
    try {
        const pkg = JSON.parse(readFileSync(join(path, "package.json"), "utf-8")) as {
            omp?: { extensions?: unknown };
            pi?: { extensions?: unknown };
        };
        const declares = (extensions: unknown) =>
            Array.isArray(extensions) &&
            extensions.length > 0 &&
            extensions.every((entry) => typeof entry === "string" && entry.trim().length > 0);
        return declares(pkg.omp?.extensions) || declares(pkg.pi?.extensions);
    } catch {
        return null;
    }
}

async function runHealthChecks(options: {
    cwd: string;
    prompts: PromptIO;
    deps: DoctorDeps;
    quiet?: boolean;
}): Promise<HealthReport> {
    const results: CheckResult[] = [];
    const repairPlan: RepairPlan = {
        installPlugin: false,
        disableCompaction: false,
        disableMemory: false,
        priorMemoryBackend: null,
        writeUserConfig: false,
        hostSupported: true,
    };
    // OMP's native settings are global, so the shared user config decides which
    // native managers count as conflicts; a manager Eidnara has switched off
    // stays on, and a project-tier disagreement is only reported.
    const eidnara = readEidnaraModes(getSharedUserConfigPath());
    const modeOverrides = projectModeOverrides(
        resolveEidnaraProjectConfigPath(options.cwd),
        eidnara,
    );
    if (modeOverrides.length > 0) {
        add(
            results,
            "warn",
            `Project config overrides ${modeOverrides.join(", ")}; OMP's native settings follow the shared config, so this project may run both Eidnara and the native manager, or neither.`,
        );
    }
    const loaded = loadPiConfig({ cwd: options.cwd });
    // OMP's config root hangs off the home directory; without one the derived paths would be
    // relative to the working directory, so they are neither compared, reported, nor scanned.
    const ompPathsAvailable = hasHomeDir();
    const omp = options.deps.detectOmpBinary();
    if (!omp) {
        add(results, "fail", "OMP binary not found on PATH or in standard user bin directories");
    } else {
        const version = options.deps.getOmpVersion(omp.path);
        if (!version) {
            repairPlan.hostSupported = false;
            add(results, "fail", `OMP at ${omp.path} could not report its version`);
        } else if (compareVersionStrings(version, MIN_OMP_VERSION) < 0) {
            repairPlan.hostSupported = false;
            add(results, "fail", `OMP ${version} is older than tested minimum ${MIN_OMP_VERSION}`);
        } else add(results, "pass", `OMP ${version} detected at ${omp.path}`);

        const plugins = options.deps.listOmpPlugins(omp.path);
        if (!plugins) {
            add(results, "fail", "`omp plugin list --json` failed or returned invalid JSON");
        } else {
            const plugin = plugins.find((entry) => entry.name === OMP_PLUGIN_PACKAGE);
            if (!plugin) {
                // Installing fetches from npm; `--force` repairs configuration only.
                add(
                    results,
                    "fail",
                    `${OMP_PLUGIN_PACKAGE} is not installed in OMP. Run \`omp plugin install ${OMP_PLUGIN_PACKAGE}\`, then re-run doctor.`,
                );
            } else if (!plugin.enabled) {
                add(results, "fail", `${OMP_PLUGIN_PACKAGE} is installed but disabled in OMP`);
                repairPlan.installPlugin = true;
            } else {
                add(results, "pass", `${OMP_PLUGIN_PACKAGE} ${plugin.version} is enabled`);
                const manifest = pluginDeclaresOmp(plugin.path);
                if (manifest === true)
                    add(results, "pass", "Plugin exposes an OMP/Pi extension manifest");
                else if (manifest === false)
                    add(results, "fail", "Installed plugin has no OMP/Pi extension manifest");
                else add(results, "warn", "Could not inspect the installed plugin manifest");
            }
        }

        const compaction = options.deps.getOmpSetting(omp.path, "compaction.enabled");
        if (compaction === false) add(results, "pass", "OMP native compaction is disabled");
        else if (compaction === true && !eidnara.compactionEnabled) {
            add(
                results,
                "pass",
                "OMP native compaction stays enabled because Eidnara compaction is off in the shared config",
            );
        } else if (compaction === true) {
            add(results, "fail", "OMP native compaction is enabled and conflicts with Eidnara");
            repairPlan.disableCompaction = true;
        } else add(results, "fail", "Could not read OMP compaction.enabled");

        const memory = options.deps.getOmpSetting(omp.path, "memory.backend");
        if (memory === "off") add(results, "pass", "OMP automatic memory backend is disabled");
        else if (typeof memory === "string" && !eidnara.memoryEnabled) {
            add(
                results,
                "pass",
                `OMP memory.backend=${memory} stays enabled because Eidnara memory is off in the shared config`,
            );
        } else if (typeof memory === "string") {
            add(
                results,
                "fail",
                `OMP memory.backend=${memory} duplicates Eidnara memory injection`,
            );
            repairPlan.disableMemory = true;
            repairPlan.priorMemoryBackend = memory;
        } else add(results, "fail", "Could not read OMP memory.backend");

        const nonGlobalSources = getOmpNonGlobalConfigSources(options.cwd);
        if (
            nonGlobalSources.length > 0 &&
            (repairPlan.disableCompaction || repairPlan.disableMemory)
        ) {
            add(
                results,
                "warn",
                "OMP project/overlay config owns effective conflicting settings; automatic global repair is disabled: " +
                    nonGlobalSources.join(", "),
            );
        }

        const reportedAgentDir = options.deps.runOmpCommand(omp.path, ["config", "path"], 10_000);
        if (!ompPathsAvailable) {
            add(
                results,
                "warn",
                "Could not verify OMP active agent directory: no home directory to resolve the expected path",
            );
        } else if (!reportedAgentDir.ok) {
            add(results, "warn", "Could not verify OMP active agent directory");
        } else {
            const reportedPath = resolve(reportedAgentDir.stdout);
            const expectedPath = resolve(getOmpAgentDir());
            if (reportedPath === expectedPath) {
                add(results, "pass", `OMP agent directory resolved to ${getOmpAgentDir()}`);
            } else {
                add(
                    results,
                    "fail",
                    `OMP reports agent directory ${reportedAgentDir.stdout}, but Eidnara resolved ${getOmpAgentDir()}`,
                );
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
                add(results, "warn", `No Eidnara user config at ${detected.path}`);
                repairPlan.writeUserConfig = true;
            } else {
                add(results, "info", `No project Eidnara config at ${detected.path}`);
            }
            continue;
        }
        // The runtime loader downgrades a malformed file to a warning and runs
        // on defaults, so the parse result is checked here to surface it as a failure.
        const parsed = readJsoncLenient(detected.path);
        if (parsed.parseError) {
            add(results, "fail", `Invalid Eidnara ${label} config: ${parsed.parseError}`);
        } else {
            add(results, "pass", `Eidnara ${label} config parses: ${basename(detected.path)}`);
        }
    }
    if (loaded.warnings.length === 0)
        add(results, "pass", "Eidnara runtime config loads successfully");
    else for (const warning of loaded.warnings.slice(0, 5)) add(results, "warn", warning);

    if (!ompPathsAvailable) {
        add(results, "fail", "OMP user-level paths are unavailable: HOME is unset or not absolute");
    } else {
        add(results, "info", `OMP config: ${getOmpConfigPath()}`);
        add(results, "info", `OMP plugin lock: ${getOmpPluginsLockPath()}`);
        add(results, "info", `OMP sessions: ${getOmpSessionsRoot()}`);
    }
    const packageDir = getOmpPackageDir();
    if (packageDir) add(results, "info", `OMP package override: ${packageDir}`);
    for (const source of getOmpNonGlobalConfigSources(options.cwd)) {
        add(results, "info", `OMP non-global config: ${source}`);
    }
    const logPath = getEidnaraLogPath("pi");
    add(
        results,
        "info",
        `Pi-compatible runtime log: ${logPath}${existsSync(logPath) ? "" : " (not created yet)"}`,
    );

    if (ompPathsAvailable) {
        const sessions = collectPiRecentSessions(getOmpSessionsRoot());
        if (sessions.status !== "ok") {
            add(
                results,
                "warn",
                `Some OMP session directories under ${getOmpSessionsRoot()} could not be read`,
            );
        }
        for (const line of describeHistorianDumps(collectPiHistorianDumps(sessions.sessions))) {
            add(results, line.status, line.message);
        }
    }

    if (!options.quiet) for (const result of results) printResult(options.prompts, result);
    return {
        results,
        repairPlan,
        pass: results.filter((result) => result.status === "pass").length,
        warn: results.filter((result) => result.status === "warn").length,
        fail: results.filter((result) => result.status === "fail").length,
    };
}

function writeDefaultConfig(path: string): void {
    mkdirSync(dirname(path), { recursive: true });
    const config = {
        $schema: "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json",
        ...EidnaraConfigSchema.parse({}),
    };
    writeFileAtomic(path, `${stringifyJsonc(config, null, 2)}\n`);
}

interface RepairOutcome {
    fixed: number;
    /** Repairs that were attempted and did not take; the doctor exits non-zero when any did. */
    failed: number;
}

async function repair(
    plan: RepairPlan,
    deps: DoctorDeps,
    prompts: PromptIO,
    cwd: string,
): Promise<RepairOutcome> {
    let fixed = 0;
    let failed = 0;
    const userConfigBase = eidnaraUserConfigBasePath();
    const userConfig = userConfigBase === undefined ? null : detectConfigFile(userConfigBase);
    if (plan.writeUserConfig && userConfig !== null && userConfig.format === "none") {
        try {
            writeDefaultConfig(userConfig.path);
            prompts.log.success(`Wrote default Eidnara config to ${userConfig.path}`);
            fixed += 1;
        } catch (error) {
            failed += 1;
            prompts.log.error(
                `Could not write default Eidnara config to ${userConfig.path}: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    }
    const omp = deps.detectOmpBinary();
    if (!omp) return { fixed, failed };
    // Enabling the plugin on an unverified host would run it beside both
    // native managers, which stay on below.
    if (!plan.hostSupported) {
        if (plan.installPlugin || plan.disableCompaction || plan.disableMemory) {
            prompts.log.error(
                `Leaving ${OMP_PLUGIN_PACKAGE} and OMP native compaction and memory as they are: this OMP is missing a version or older than ${MIN_OMP_VERSION}, so the plugin may not run. Upgrade with \`omp update\` first.`,
            );
        }
        return { fixed, failed };
    }
    const wantsManagersOff = plan.disableCompaction || plan.disableMemory;
    // Global settings are unobservable under project or overlay config, so the
    // manager repairs would be refused below; enabling the plugin first would
    // then leave it running beside both native managers.
    const nonGlobalSources = getOmpNonGlobalConfigSources(cwd);
    if (wantsManagersOff && nonGlobalSources.length > 0) {
        prompts.log.error(
            `Leaving OMP as it is: effective settings include ${nonGlobalSources.join(", ")}, so the global compaction and memory settings cannot be changed safely`,
        );
        return { fixed, failed };
    }
    let enabledHere = false;
    if (plan.installPlugin) {
        // `pluginDeclaresOmp` returns `null` for a plugin without a readable
        // manifest; only a verified manifest counts, so a `null` also blocks.
        const installed = deps
            .listOmpPlugins(omp.path)
            ?.find((entry) => entry.name === OMP_PLUGIN_PACKAGE);
        if (pluginDeclaresOmp(installed?.path) !== true) {
            prompts.log.error(
                `Leaving ${OMP_PLUGIN_PACKAGE} disabled: its install${installed ? ` at ${installed.path}` : ""} has no verifiable OMP/Pi extension manifest, so OMP would load a package that is not an extension`,
            );
            return { fixed, failed };
        }
        const result = await deps.ensurePluginEntry();
        if (result.ok) {
            prompts.log.success(result.message);
            fixed += 1;
            enabledHere = true;
        } else prompts.log.error(result.message);
    }
    if (!wantsManagersOff) return { fixed, failed };
    // A plugin enabled in this run beside a native manager that stayed on
    // would run both after restart, so any later failure restores the prior
    // disabled state.
    const disablePluginAgain = (why: string): void => {
        if (!enabledHere) return;
        const rollback = deps.runOmpCommand(
            omp.path,
            ["plugin", "disable", OMP_PLUGIN_PACKAGE],
            120_000,
        );
        if (rollback.ok) {
            fixed -= 1;
            prompts.log.warn(`Disabled ${OMP_PLUGIN_PACKAGE} again: ${why}`);
        } else {
            prompts.log.error(
                `Could not disable ${OMP_PLUGIN_PACKAGE} after ${why} (${rollback.stderr || rollback.stdout || "omp exited with an error"}). Run \`omp plugin disable ${OMP_PLUGIN_PACKAGE}\` by hand.`,
            );
        }
    };
    // Turning off OMP's managers only makes sense once the plugin that
    // replaces them is enabled and carries an extension manifest.
    const plugin = deps
        .listOmpPlugins(omp.path)
        ?.find((entry) => entry.name === OMP_PLUGIN_PACKAGE);
    if (plugin?.enabled !== true || pluginDeclaresOmp(plugin.path) !== true) {
        prompts.log.error(
            `Leaving OMP native compaction and memory on: ${OMP_PLUGIN_PACKAGE} is not enabled in OMP with a verified extension manifest, so nothing would replace them`,
        );
        disablePluginAgain("its enabled state could not be verified afterwards");
        return { fixed, failed };
    }
    // Each mutation records the value that undoes it, so a later failure can
    // restore every manager already turned off in this run.
    const applied: Array<{ key: string; prior: string }> = [];
    let failedKey: string | null = null;
    for (const [enabled, key, value, prior] of [
        [plan.disableCompaction, "compaction.enabled", "false", "true"],
        [plan.disableMemory, "memory.backend", "off", plan.priorMemoryBackend],
    ] as const) {
        if (!enabled) continue;
        const result = deps.runOmpCommand(omp.path, ["config", "set", key, value], 10_000);
        if (result.ok) {
            prompts.log.success(`Set OMP ${key}=${value}`);
            fixed += 1;
            if (prior !== null) applied.push({ key, prior });
        } else {
            failedKey = key;
            prompts.log.error(result.stderr || `Could not set OMP ${key}`);
            break;
        }
    }
    if (failedKey === null) return { fixed, failed };
    for (const { key, prior } of applied.reverse()) {
        const restore = deps.runOmpCommand(omp.path, ["config", "set", key, prior], 10_000);
        if (restore.ok) {
            fixed -= 1;
            prompts.log.warn(
                `Restored OMP ${key}=${prior}: setting ${failedKey} failed afterwards`,
            );
        } else {
            prompts.log.error(
                `Could not restore OMP ${key}=${prior} after setting ${failedKey} failed (${restore.stderr || restore.stdout || "omp exited with an error"}). Run \`omp config set ${key} ${prior}\` by hand.`,
            );
        }
    }
    disablePluginAgain(
        `OMP ${failedKey} could not be turned off, so leaving it enabled would run both`,
    );

    return { fixed, failed };
}

function timestamp(date: Date): string {
    return date
        .toISOString()
        .replace(/[-:]/g, "")
        .replace(/\.\d{3}Z$/, "Z");
}

async function runIssueFlow(options: {
    cwd: string;
    prompts: PromptIO;
    deps: DoctorDeps;
}): Promise<number> {
    const title = await options.prompts.text("Issue title", {
        placeholder: "Short summary of the OMP problem",
        validate: (value) => (value.trim() ? undefined : "Title is required"),
    });
    const description = await options.prompts.text("Issue description", {
        placeholder: "What happened, expected behavior, and reproduction steps",
        validate: (value) => (value.trim() ? undefined : "Description is required"),
    });
    const report = await runHealthChecks({ ...options, quiet: true });
    const body = [
        "## Description",
        sanitizeDiagnosticText(description),
        "",
        "## OMP diagnostics",
        `- Eidnara CLI: ${selfVersion()}`,
        ...report.results.map(
            (result) =>
                `- ${result.status.toUpperCase()}: ${sanitizeDiagnosticText(result.message)}`,
        ),
    ].join("\n");
    const path = writeNewFile(
        join(options.cwd, `eidnara-omp-issue-${timestamp(options.deps.now())}`),
        `${capBodyToGithubLimit(body)}\n`,
    );
    options.prompts.log.success(`Sanitized report written to ${path}`);
    try {
        options.deps.execFileSync("gh", ["--version"], { stdio: "ignore" });
    } catch {
        options.prompts.log.info("gh CLI unavailable; submit the generated report manually");
        return 0;
    }
    try {
        options.deps.execFileSync("gh", ["auth", "status"], { stdio: "ignore" });
    } catch {
        options.prompts.log.info(
            "gh CLI is installed but not authenticated; submit the generated report manually",
        );
        return 0;
    }
    if (await options.prompts.confirm("Submit this issue on GitHub now?", false)) {
        const result = options.deps.spawnSync(
            "gh",
            [
                "issue",
                "create",
                "-R",
                "ahrav/eidnara",
                "--title",
                `[omp] ${sanitizeDiagnosticText(title)}`,
                "--body-file",
                path,
            ],
            { encoding: "utf-8", stdio: ["ignore", "pipe", "pipe"] },
        );
        if (result.status === 0) options.prompts.log.success(String(result.stdout).trim());
        else options.prompts.log.warn(String(result.stderr).trim());
    }
    return 0;
}

export async function runDoctor(options: RunOmpDoctorOptions = {}): Promise<number> {
    const deps: DoctorDeps = {
        ...DEFAULT_DEPS,
        prompts: options.prompts ?? DEFAULT_DEPS.prompts,
        ...options.deps,
    };
    const prompts = options.prompts ?? deps.prompts;
    const cwd = options.cwd ?? process.cwd();
    if (options.issue) return runIssueFlow({ cwd, prompts, deps });

    prompts.intro("Eidnara for Oh My Pi (OMP) Doctor");
    const first = await runHealthChecks({ cwd, prompts, deps });
    prompts.log.message(`Summary: PASS ${first.pass} / WARN ${first.warn} / FAIL ${first.fail}`);
    if (!options.force) return first.fail === 0 ? 0 : 1;
    if (first.fail === 0 && !first.repairPlan.writeUserConfig) return 0;
    const repaired = await repair(first.repairPlan, deps, prompts, cwd);
    prompts.log.info(
        repaired.failed > 0
            ? `Applied ${repaired.fixed} repair(s), ${repaired.failed} failed; re-checking`
            : `Applied ${repaired.fixed} repair(s); re-checking`,
    );
    const second = await runHealthChecks({ cwd, prompts, deps });
    prompts.log.message(`Summary: PASS ${second.pass} / WARN ${second.warn} / FAIL ${second.fail}`);
    if (repaired.failed > 0) {
        prompts.log.error("Doctor could not complete the requested repair");
        return 1;
    }
    return second.fail === 0 ? 0 : 1;
}
