import { afterEach, describe, expect, it, setDefaultTimeout } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { parse as parseJsonc } from "comment-json";
import type { PiDiagnosticReport } from "../lib/diagnostics-pi";
import type { PromptIO, PromptSpinner, SelectOption } from "../lib/prompts";
import { type RunDoctorOptions, runDoctor } from "./doctor-pi";

setDefaultTimeout(30_000);

const tempRoots: string[] = [];
const originalHome = process.env.HOME;
const originalPiDir = process.env.PI_CODING_AGENT_DIR;
const originalDataHome = process.env.XDG_DATA_HOME;
const originalConfigHome = process.env.XDG_CONFIG_HOME;
const originalLogPath = process.env.EIDNARA_LOG_PATH;

function makeTempRoot(prefix = "eidnara-pi-doctor-"): string {
    const path = mkdtempSync(join(tmpdir(), prefix));
    tempRoots.push(path);
    return path;
}

class MockPrompts implements PromptIO {
    readonly messages: string[] = [];
    private readonly texts: string[];
    private readonly confirms: boolean[];

    constructor(options: { texts?: string[]; confirms?: boolean[] } = {}) {
        this.texts = [...(options.texts ?? [])];
        this.confirms = [...(options.confirms ?? [])];
    }

    readonly log = {
        info: (message: string) => this.messages.push(`info:${message}`),
        success: (message: string) => this.messages.push(`success:${message}`),
        warn: (message: string) => this.messages.push(`warn:${message}`),
        message: (message: string) => this.messages.push(`message:${message}`),
    };

    intro(message: string): void {
        this.messages.push(`intro:${message}`);
    }

    outro(message: string): void {
        this.messages.push(`outro:${message}`);
    }

    note(message: string, title?: string): void {
        this.messages.push(`note:${title ?? ""}:${message}`);
    }

    spinner(): PromptSpinner {
        return {
            start: (message: string) => this.messages.push(`spinner-start:${message}`),
            stop: (message: string) => this.messages.push(`spinner-stop:${message}`),
        };
    }

    async confirm(): Promise<boolean> {
        return this.confirms.shift() ?? false;
    }

    async text(_message: string, options = {}): Promise<string> {
        return this.texts.shift() ?? options.initialValue ?? "mock text";
    }

    async selectOne(_message: string, options: SelectOption[]): Promise<string> {
        return options.find((option) => option.recommended)?.value ?? options[0].value;
    }
}

function setEnv(root: string, cwd: string): string {
    process.env.HOME = root;
    process.env.PI_CODING_AGENT_DIR = join(root, ".pi", "agent");
    process.env.XDG_DATA_HOME = join(root, ".local", "share");
    process.env.XDG_CONFIG_HOME = join(root, ".config");
    mkdirSync(process.env.PI_CODING_AGENT_DIR, { recursive: true });
    mkdirSync(join(process.env.XDG_CONFIG_HOME, "eidnara"), { recursive: true });
    mkdirSync(join(cwd, ".eidnara"), { recursive: true });
    return process.env.PI_CODING_AGENT_DIR;
}

function writeHealthyFiles(agentDir: string, cwd: string): void {
    writeFileSync(
        join(agentDir, "settings.json"),
        JSON.stringify({
            packages: ["npm:@eidnara/pi", "npm:other-pi-extension"],
        }),
    );
    const configHome = process.env.XDG_CONFIG_HOME ?? join(process.env.HOME ?? "", ".config");
    writeFileSync(join(configHome, "eidnara", "eidnara.jsonc"), JSON.stringify({ enabled: true }));
    writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), JSON.stringify({ enabled: true }));
}

function baseOptions(root: string, cwd: string, prompts: MockPrompts): RunDoctorOptions {
    return {
        cwd,
        prompts,
        deps: {
            detectPiBinary: () => ({
                path: join(root, ".pi", "bin", "pi"),
                source: "home",
            }),
            getPiVersion: () => "0.74.0",
            now: () => new Date("2026-04-28T12:34:56Z"),
            execFileSync: () => {
                throw new Error("gh unavailable");
            },
            spawnSync: () => ({ status: 1, stdout: "", stderr: "not expected" }) as never,
        },
    };
}

afterEach(() => {
    if (originalHome === undefined) delete process.env.HOME;
    else process.env.HOME = originalHome;
    if (originalPiDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
    else process.env.PI_CODING_AGENT_DIR = originalPiDir;
    if (originalDataHome === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = originalDataHome;
    if (originalConfigHome === undefined) delete process.env.XDG_CONFIG_HOME;
    else process.env.XDG_CONFIG_HOME = originalConfigHome;
    if (originalLogPath === undefined) delete process.env.EIDNARA_LOG_PATH;
    else process.env.EIDNARA_LOG_PATH = originalLogPath;

    for (const path of tempRoots.splice(0)) {
        rmSync(path, { recursive: true, force: true });
    }
});

describe("Pi doctor", () => {
    it("passes Phase 1 with a healthy mocked environment", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const prompts = new MockPrompts();

        const code = await runDoctor(baseOptions(root, cwd, prompts));

        expect(code).toBe(0);
        const output = prompts.messages.join("\n");
        expect(output).toContain("PASS Pi 0.74.0 detected");
        expect(output).toContain("PASS npm:@eidnara/pi is registered");
        expect(output).toContain("PASS No conflicting Eidnara entries in Pi packages[]");
        expect(output).toContain("INFO Other Pi extensions registered: npm:other-pi-extension");
        expect(output).toContain("Summary: PASS");
        expect(output).toContain("FAIL 0");
    });

    it("repairs missing package entry and missing user config in --force mode", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [] }));
        writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), JSON.stringify({ enabled: true }));
        const prompts = new MockPrompts();

        const code = await runDoctor({
            ...baseOptions(root, cwd, prompts),
            force: true,
        });

        expect(code).toBe(0);
        const settings = parseJsonc(readFileSync(join(agentDir, "settings.json"), "utf-8")) as {
            packages?: string[];
        };
        expect(settings.packages).toContain("npm:@eidnara/pi");
        expect(existsSync(join(root, ".config", "eidnara", "eidnara.jsonc"))).toBe(true);
        const output = prompts.messages.join("\n");
        expect(output).toContain("Added npm:@eidnara/pi");
        expect(output).toContain("Wrote default Eidnara config");
        expect(output).toContain("Repair attempted; 2 item(s) changed");
    });

    it("generates a sanitized markdown report in --issue mode without calling gh create", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        writeFileSync(
            join(tmpdir(), "eidnara.log"),
            `token=abc123\nUser path ${root}/secret with sk-12345678901234567890\n`,
        );
        const ghCalls: Array<{ command: string; args: readonly string[] }> = [];
        const logged: unknown[] = [];
        const originalConsoleLog = console.log;
        console.log = (...args: unknown[]) => {
            logged.push(...args);
        };
        const prompts = new MockPrompts({
            texts: ["Bug title", `Failure in ${root}`],
        });
        const diagnosticReport: PiDiagnosticReport = {
            timestamp: "2026-04-28T12:34:56.000Z",
            platform: "darwin",
            arch: "arm64",
            nodeVersion: "v24.0.0",
            pluginVersion: "0.1.0",
            piInstalled: true,
            piPath: join(root, ".pi", "bin", "pi"),
            piVersion: "0.74.0",
            settings: {
                path: join(agentDir, "settings.json"),
                exists: true,
                hasEidnaraPackage: true,
                packages: ["npm:@eidnara/pi"],
            },
            configPaths: {
                agentDir,
                userConfig: join(root, ".config", "eidnara", "eidnara.jsonc"),
                projectConfig: join(cwd, ".eidnara", "eidnara.jsonc"),
            },
            userConfig: {
                path: join(root, ".config", "eidnara", "eidnara.jsonc"),
                exists: true,
                flags: { enabled: true },
            },
            projectConfig: {
                path: join(cwd, ".eidnara", "eidnara.jsonc"),
                exists: true,
                flags: { enabled: true },
            },
            loadedConfigPaths: ["<HOME>/.config/eidnara/eidnara.jsonc"],
            loadWarnings: [],
            conflicts: { knownConflicts: [], otherPiExtensions: [] },
            logFile: {
                path: join(tmpdir(), "eidnara.log"),
                exists: true,
                sizeKb: 1,
            },
            recentSessions: [],
            historianDumps: {
                byProject: [],
                legacyDumps: {
                    dir: join(tmpdir(), "pi", "context", "historian"),
                    count: 0,
                    recent: [],
                },
            },
        };

        const options = baseOptions(root, cwd, prompts);
        try {
            const code = await runDoctor({
                ...options,
                issue: true,
                deps: {
                    ...options.deps,
                    collectDiagnostics: async () => diagnosticReport,
                    execFileSync: () => {
                        throw new Error("gh unavailable");
                    },
                    spawnSync: ((command: string, args: readonly string[]) => {
                        ghCalls.push({ command, args });
                        return { status: 0, stdout: "", stderr: "" };
                    }) as never,
                },
            });

            expect(code).toBe(0);
        } finally {
            console.log = originalConsoleLog;
        }

        expect(ghCalls).toEqual([]);
        expect(logged.join("\n")).toContain("[pi] Bug title");
        const reportPath = join(cwd, "eidnara-pi-issue-20260428-123456.md");
        expect(existsSync(reportPath)).toBe(true);
        const report = readFileSync(reportPath, "utf-8");
        expect(report).toContain("[pi] Bug title");
        expect(report).toContain("<HOME>");
        expect(report).not.toContain(root);
        expect(report).not.toContain("abc123");
        expect(report).not.toContain("sk-12345678901234567890");
    });

    it("sanitizes the issue title before passing it to gh issue create", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const ghCalls: Array<{ command: string; args: readonly string[] }> = [];
        const originalConsoleLog = console.log;
        console.log = () => {};
        const prompts = new MockPrompts({
            texts: [`Crash in ${root}/private token=abc123`, "Description"],
            confirms: [true],
        });
        const diagnosticReport: PiDiagnosticReport = {
            timestamp: "2026-04-28T12:34:56.000Z",
            platform: "linux",
            arch: "x64",
            nodeVersion: "v24.0.0",
            pluginVersion: "0.1.0",
            piInstalled: true,
            piPath: join(root, ".pi", "bin", "pi"),
            piVersion: "0.74.0",
            settings: {
                path: join(agentDir, "settings.json"),
                exists: true,
                hasEidnaraPackage: true,
                packages: ["npm:@eidnara/pi"],
            },
            configPaths: {
                agentDir,
                userConfig: join(root, ".config", "eidnara", "eidnara.jsonc"),
                projectConfig: join(cwd, ".eidnara", "eidnara.jsonc"),
            },
            userConfig: {
                path: join(root, ".config", "eidnara", "eidnara.jsonc"),
                exists: true,
                flags: {},
            },
            projectConfig: {
                path: join(cwd, ".eidnara", "eidnara.jsonc"),
                exists: true,
                flags: {},
            },
            loadedConfigPaths: [],
            loadWarnings: [],
            conflicts: { knownConflicts: [], otherPiExtensions: [] },
            logFile: { path: join(root, "missing.log"), exists: false, sizeKb: 0 },
            recentSessions: [],
            historianDumps: {
                byProject: [],
                legacyDumps: { dir: join(root, "dumps"), count: 0, recent: [] },
            },
        };

        const options = baseOptions(root, cwd, prompts);
        try {
            const code = await runDoctor({
                ...options,
                issue: true,
                deps: {
                    ...options.deps,
                    collectDiagnostics: async () => diagnosticReport,
                    execFileSync: (() => "") as never,
                    spawnSync: ((command: string, args: readonly string[]) => {
                        ghCalls.push({ command, args });
                        return { status: 0, stdout: "https://github.com/x/1", stderr: "" };
                    }) as never,
                },
            });
            expect(code).toBe(0);
        } finally {
            console.log = originalConsoleLog;
        }

        expect(ghCalls).toHaveLength(1);
        const titleArg = ghCalls[0]?.args[ghCalls[0].args.indexOf("--title") + 1] ?? "";
        expect(titleArg).toStartWith("[pi] Crash in ");
        expect(titleArg).not.toContain(root);
        expect(titleArg).not.toContain("abc123");
    });

    it("reports an unknown CLI version as info instead of pass-current", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const prompts = new MockPrompts();
        const options = baseOptions(root, cwd, prompts);

        const code = await runDoctor({
            ...options,
            deps: {
                ...options.deps,
                selfVersion: () => "unknown",
            },
        });

        expect(code).toBe(0);
        const output = prompts.messages.join("\n");
        expect(output).toContain("INFO Eidnara for Pi CLI version unknown");
        expect(output).not.toContain("PASS Eidnara for Pi CLI");
    });

    it("fails when `pi --version` prints output that is not a version", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const prompts = new MockPrompts();
        const options = baseOptions(root, cwd, prompts);
        const stderr: string[] = [];
        const originalConsoleError = console.error;
        console.error = (...args: unknown[]) => {
            stderr.push(args.map(String).join(" "));
        };

        let code: number;
        try {
            code = await runDoctor({
                ...options,
                deps: {
                    ...options.deps,
                    getPiVersion: () =>
                        "node:internal/modules/cjs/loader:1228\n  throw err;\nError: Cannot find module",
                },
            });
        } finally {
            console.error = originalConsoleError;
        }

        expect(code).toBe(1);
        const output = prompts.messages.join("\n");
        expect(output).not.toContain("PASS Pi version meets minimum");
        expect(output).not.toContain("detected at");
        expect(stderr.join("\n")).toContain("FAIL Pi CLI at");
        expect(stderr.join("\n")).toContain("printed unrecognized version output");
        expect(stderr.join("\n")).not.toContain("\n  throw err;");
    });

    it("does not write a default eidnara.jsonc over an existing eidnara.json in --force mode", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const configDir = join(root, ".config", "eidnara");
        rmSync(join(configDir, "eidnara.jsonc"));
        writeFileSync(join(configDir, "eidnara.json"), JSON.stringify({ enabled: false }));
        const prompts = new MockPrompts();

        const code = await runDoctor({ ...baseOptions(root, cwd, prompts), force: true });

        expect(code).toBe(0);
        expect(existsSync(join(configDir, "eidnara.jsonc"))).toBe(false);
        const output = prompts.messages.join("\n");
        expect(output).toContain("PASS user eidnara.json is valid JSONC");
        expect(output).not.toContain("No user eidnara.jsonc found");
        expect(output).not.toContain("Wrote default Eidnara config");
        expect(output).toContain("Repair attempted; 0 item(s) changed");
    });

    it("reads the last log line without loading a log larger than the tail window", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const logPath = join(root, "eidnara.log");
        const filler = `${"x".repeat(1023)}\n`.repeat(200);
        writeFileSync(logPath, `${filler}final line marker\n\n`);
        process.env.EIDNARA_LOG_PATH = logPath;
        const prompts = new MockPrompts();

        const code = await runDoctor(baseOptions(root, cwd, prompts));

        expect(code).toBe(0);
        const output = prompts.messages.join("\n");
        expect(output).toContain(`Log file: ${logPath} (200 KB)`);
        expect(output).toContain("Last plugin log line: final line marker");
    });

    it("treats a local checkout of @eidnara/pi as registered and does not add the npm entry", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const checkout = join(root, "eidnara-pi-checkout");
        mkdirSync(checkout, { recursive: true });
        writeFileSync(join(checkout, "package.json"), JSON.stringify({ name: "@eidnara/pi" }));
        writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [checkout] }));
        const prompts = new MockPrompts();

        const code = await runDoctor({ ...baseOptions(root, cwd, prompts), force: true });

        expect(code).toBe(0);
        const settings = parseJsonc(readFileSync(join(agentDir, "settings.json"), "utf-8")) as {
            packages?: unknown[];
        };
        expect(settings.packages).toEqual([checkout]);
        const output = prompts.messages.join("\n");
        expect(output).toContain("PASS npm:@eidnara/pi is registered in packages[]");
        expect(output).toContain("PASS No conflicting Eidnara entries in Pi packages[]");
        expect(output).not.toContain("Added npm:@eidnara/pi");
    });

    it("fails when both a local checkout and the npm entry are registered", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeHealthyFiles(agentDir, cwd);
        const checkout = join(root, "eidnara-pi-checkout");
        mkdirSync(checkout, { recursive: true });
        writeFileSync(join(checkout, "package.json"), JSON.stringify({ name: "@eidnara/pi" }));
        writeFileSync(
            join(agentDir, "settings.json"),
            JSON.stringify({ packages: [checkout, "npm:@eidnara/pi"] }),
        );
        const prompts = new MockPrompts();
        const originalConsoleError = console.error;
        const stderr: string[] = [];
        console.error = (...args: unknown[]) => {
            stderr.push(args.map(String).join(" "));
        };

        let code: number;
        try {
            code = await runDoctor(baseOptions(root, cwd, prompts));
        } finally {
            console.error = originalConsoleError;
        }

        expect(code).toBe(1);
        expect(stderr.join("\n")).toContain("Multiple Eidnara entries in Pi packages[]");
    });

    it("exits non-zero when --force cannot write the default user config", async () => {
        const root = makeTempRoot();
        const cwd = makeTempRoot("eidnara-pi-doctor-cwd-");
        const agentDir = setEnv(root, cwd);
        writeFileSync(
            join(agentDir, "settings.json"),
            JSON.stringify({ packages: ["npm:@eidnara/pi"] }),
        );
        writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), JSON.stringify({ enabled: true }));
        // A regular file where the config directory belongs makes `mkdirSync` fail.
        const configDir = join(root, ".config", "eidnara");
        rmSync(configDir, { recursive: true });
        writeFileSync(configDir, "not a directory");
        const prompts = new MockPrompts();
        const stderr: string[] = [];
        const originalConsoleError = console.error;
        console.error = (...args: unknown[]) => {
            stderr.push(args.map(String).join(" "));
        };

        let code: number;
        try {
            code = await runDoctor({ ...baseOptions(root, cwd, prompts), force: true });
        } finally {
            console.error = originalConsoleError;
        }

        expect(code).toBe(1);
        expect(stderr.join("\n")).toContain("FAIL Could not write");
        const output = prompts.messages.join("\n");
        expect(output).toContain("Repair attempted; 0 item(s) changed, 1 item(s) failed");
        expect(output).toContain("outro:Doctor could not complete the requested repair");
        expect(output).not.toContain("Doctor repair complete");
    });
});
