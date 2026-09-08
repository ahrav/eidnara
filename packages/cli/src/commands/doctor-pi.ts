import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import { resolveEidnaraProjectConfigPath } from "@eidnara/opencode/config/config-paths";
import { EidnaraConfigSchema } from "@eidnara/opencode/config/schema/eidnara";
import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";
import { loadPiConfig } from "@eidnara/pi/config";
import { stringify as stringifyJsonc } from "comment-json";

import { writeFileAtomic } from "../lib/atomic-write";
import { collectDiagnostics } from "../lib/diagnostics-pi";
import { readJsoncLenient } from "../lib/jsonc-config";
import { bundleIssueReport } from "../lib/logs-pi";
import { getEidnaraLogPath, getPiUserExtensionsPath, getSharedUserConfigPath } from "../lib/paths";
import {
    detectPiBinary,
    getPiVersion,
    PI_PACKAGE_SOURCE,
    type PiBinaryInfo,
} from "../lib/pi-helpers";
import { type PromptIO, promptIO } from "../lib/prompts";
import { writePiSettingsPackage } from "./setup-pi";

// Pi 0.74.0 changed the package scope from `@mariozechner/pi-coding-agent` to `@earendil-works/pi-coding-agent`; older Pi versions cannot load this extension because its peerDependency uses the new scope.
const MIN_PI_VERSION = "0.74.0";

type CheckStatus = "pass" | "warn" | "fail" | "info";

interface CheckResult {
    status: CheckStatus;
    message: string;
}

interface RepairPlan {
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
    help?: boolean;
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

function printDoctorHelp(): void {
    console.log("");
    console.log("  Eidnara for Pi doctor");
    console.log("  ───────────────────────────");
    console.log("");
    console.log("  Usage:");
    console.log("    eidnara doctor --harness pi          Run health checks");
    console.log("    eidnara doctor --harness pi --force  Repair safe issues, then re-check");
    console.log("    eidnara doctor --harness pi --issue  Create a sanitized bug report");
    console.log("    eidnara doctor --harness pi --help   Show this help");
    console.log("");
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

function parseSemver(version: string | null): [number, number, number] | null {
    if (!version) return null;
    const match = version.match(/(\d+)\.(\d+)\.(\d+)/);
    if (!match) return null;
    return [Number(match[1]), Number(match[2]), Number(match[3])];
}

function compareSemver(a: string | null, b: string): number | null {
    const left = parseSemver(a);
    const right = parseSemver(b);
    if (!left || !right) return null;
    for (let i = 0; i < 3; i += 1) {
        if (left[i] < right[i]) return -1;
        if (left[i] > right[i]) return 1;
    }
    return 0;
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

function isPiEidnaraPackageEntry(entry: unknown): boolean {
    return entry === PI_PACKAGE_SOURCE;
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
        addPackageEntry: false,
        writeUserConfig: false,
    };
    const self = options.deps.selfVersion();

    const pi = options.deps.detectPiBinary();
    if (!pi) {
        add(results, "fail", "Pi binary not found on PATH or at ~/.pi/bin/pi");
    } else {
        const version = options.deps.getPiVersion(pi.path);
        if (version === null) {
            add(results, "fail", `Pi CLI was found at ${pi.path} but could not be executed`);
        } else {
            add(results, "pass", `Pi ${version} detected at ${pi.path}`);
        }
        const compare = compareSemver(version, MIN_PI_VERSION);
        if (compare !== null && compare < 0) {
            add(
                results,
                "fail",
                `Pi ${version} is older than required ${MIN_PI_VERSION}. Subagents (historian/dreamer/sidekick) use the long-form \`--extension\` flag introduced in Pi 0.71.0; older versions hard-fail with "Unknown option". Run \`pi update\` (or \`npm install -g @earendil-works/pi-coding-agent@latest\`).`,
            );
        } else if (version) {
            add(results, "pass", `Pi version meets minimum ${MIN_PI_VERSION} requirement`);
        }
    }

    if (parseSemver(self)) {
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

    const userConfigPath = getSharedUserConfigPath();
    const projectPath = resolveEidnaraProjectConfigPath(options.cwd);
    for (const [label, path, required] of [
        ["user", userConfigPath, true],
        ["project", projectPath, false],
    ] as const) {
        if (!existsSync(path)) {
            if (required) {
                add(results, "warn", `No ${label} eidnara.jsonc found at ${path}`);
                repairPlan.writeUserConfig = true;
            } else {
                add(results, "info", `No project Eidnara config found at ${path}`);
            }
            continue;
        }
        const parsed = readJsoncLenient(path);
        if (parsed.parseError)
            add(results, "fail", `${label} eidnara.jsonc is invalid JSONC: ${parsed.parseError}`);
        else add(results, "pass", `${label} eidnara.jsonc is valid JSONC: ${path}`);
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
        const stat = statSync(logPath);
        const sizeKb = (stat.size / 1024).toFixed(0);
        const lines = readFileSync(logPath, "utf-8")
            .split(/\r?\n/)
            .map((line) => line.trim())
            .filter(Boolean);
        add(results, "info", `Log file: ${logPath} (${sizeKb} KB)`);
        const lastLine = lines.at(-1);
        add(
            results,
            "info",
            `Last plugin log line: ${lastLine ? sanitizeDiagnosticText(lastLine) : "<empty log>"}`,
        );
    } else {
        add(results, "info", `No plugin log file yet at ${logPath}`);
    }

    const diagnosticsForDumps = await collectDiagnostics(options.cwd);
    const dumpBuckets = diagnosticsForDumps.historianDumps.byProject;
    if (dumpBuckets.length > 0) {
        const totalCount = dumpBuckets.reduce((sum, b) => sum + b.count, 0);
        add(
            results,
            "warn",
            `Historian debug dumps: ${totalCount} file(s) across ${dumpBuckets.length} project(s)`,
        );
        for (const bucket of dumpBuckets) {
            add(results, "info", `  [${bucket.directory}] ${bucket.count} file(s)`);
            for (const dump of bucket.recent.slice(0, 3)) {
                const age = dump.ageMinutes;
                const ageStr = age < 60 ? `${age}m ago` : `${Math.round(age / 60)}h ago`;
                add(results, "info", `    ${dump.name} (${ageStr})`);
            }
            if (bucket.count > 3) {
                add(results, "info", `    ... and ${bucket.count - 3} more`);
            }
        }
    }
    const legacyDumps = diagnosticsForDumps.historianDumps.legacyDumps;
    if (legacyDumps.count > 0) {
        add(
            results,
            "info",
            `Legacy historian dumps (pre-v0.18.x): ${legacyDumps.count} file(s) in ${legacyDumps.dir}`,
        );
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

function repair(plan: RepairPlan, prompts: PromptIO): number {
    let fixed = 0;
    if (plan.addPackageEntry) {
        const settingsPath = getPiUserExtensionsPath();
        try {
            const added = writePiSettingsPackage(settingsPath);
            prompts.log.success(
                added
                    ? `Added ${PI_PACKAGE_SOURCE} to ${settingsPath}`
                    : `${PI_PACKAGE_SOURCE} already present in ${settingsPath}`,
            );
            fixed += added ? 1 : 0;
        } catch (error) {
            console.error(
                `FAIL Could not update ${settingsPath}: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    }

    if (plan.writeUserConfig) {
        const configPath = getSharedUserConfigPath();
        if (!existsSync(configPath)) {
            try {
                writeDefaultEidnaraConfig(configPath);
                prompts.log.success(`Wrote default Eidnara config to ${configPath}`);
                fixed += 1;
            } catch (error) {
                console.error(
                    `FAIL Could not write ${configPath}: ${error instanceof Error ? error.message : String(error)}`,
                );
            }
        }
    }

    return fixed;
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

        let sessionFilter: string | null = null;
        if (report.recentSessions.length > 1) {
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

        spinner.start("Bundling Pi issue report");
        const bundled = await bundleIssueReport(report, description, title, {
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
                        `[pi] ${title}`,
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

    if (options.help) {
        printDoctorHelp();
        return 0;
    }

    if (options.issue) {
        return runIssueFlow({ cwd, prompts, deps });
    }

    prompts.intro("Eidnara for Pi Doctor");
    const first = await runHealthChecks({ cwd, prompts, deps });
    console.log("");
    prompts.log.message(`Summary: PASS ${first.pass} / WARN ${first.warn} / FAIL ${first.fail}`);

    if (options.force) {
        const fixed = repair(first.repairPlan, prompts);
        console.log("");
        prompts.log.message(
            `Repair attempted; ${fixed} item(s) changed. Re-running health checks.`,
        );
        const second = await runHealthChecks({ cwd, prompts, deps });
        console.log("");
        prompts.log.message(
            `Summary: PASS ${second.pass} / WARN ${second.warn} / FAIL ${second.fail}`,
        );
        prompts.outro(
            second.fail > 0 ? "Doctor found failures after repair" : "Doctor repair complete",
        );
        return second.fail > 0 ? 1 : 0;
    }

    prompts.outro(first.fail > 0 ? "Doctor found failures" : "Doctor complete");
    return first.fail > 0 ? 1 : 0;
}

export function parseDoctorArgs(args: string[]): RunDoctorOptions {
    return {
        force: args.includes("--force"),
        issue: args.includes("--issue"),
        help: args.includes("--help") || args.includes("-h"),
    };
}
