import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { parse as parseJsonc } from "comment-json";
import { getSharedUserConfigPath } from "../lib/paths";
import type { PromptIO, PromptSpinner, SelectOption } from "../lib/prompts";
import {
    type PiCompatibleSetupHost,
    removePiSettingsPackage,
    runSetup,
    type SetupEnvironment,
    writePiSettingsPackage,
} from "./setup-pi";

const tempRoots: string[] = [];
const originalHome = process.env.HOME;
const originalPiDir = process.env.PI_CODING_AGENT_DIR;
const originalConfigHome = process.env.XDG_CONFIG_HOME;
const originalFetch = globalThis.fetch;
const fetchCalls: unknown[] = [];

function makeTempRoot(): string {
    const path = mkdtempSync(join(tmpdir(), "eidnara-pi-setup-"));
    tempRoots.push(path);
    return path;
}

function setConfigEnv(root: string, agentDir: string): void {
    process.env.HOME = root;
    process.env.PI_CODING_AGENT_DIR = agentDir;
    process.env.XDG_CONFIG_HOME = join(root, ".config");
}

class MockPrompts implements PromptIO {
    readonly messages: string[] = [];
    private readonly confirms: boolean[];
    private readonly texts: string[];

    constructor(options: { confirms: boolean[]; texts?: string[] }) {
        this.confirms = [...options.confirms];
        this.texts = [...(options.texts ?? [])];
    }

    readonly log = {
        info: (message: string) => this.messages.push(`info:${message}`),
        success: (message: string) => this.messages.push(`success:${message}`),
        warn: (message: string) => this.messages.push(`warn:${message}`),
        error: (message: string) => this.messages.push(`error:${message}`),
        message: (message: string) => this.messages.push(`message:${message}`),
        step: (message: string) => this.messages.push(`step:${message}`),
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
        const next = this.confirms.shift();
        if (next === undefined) throw new Error("No mock confirm response queued");
        return next;
    }

    async text(_message: string, options = {}): Promise<string> {
        return this.texts.shift() ?? options.initialValue ?? "";
    }

    async selectOne(_message: string, options: SelectOption[]): Promise<string> {
        const recommended = options.find((option) => option.recommended);
        return (recommended ?? options[0]).value;
    }

    async selectAutocomplete(_message: string, options: SelectOption[]): Promise<string> {
        const recommended = options.find((option) => option.recommended);
        return (recommended ?? options[0]).value;
    }
}

beforeEach(() => {
    fetchCalls.length = 0;
    globalThis.fetch = ((...args: unknown[]) => {
        fetchCalls.push(args);
        throw new Error("network call");
    }) as unknown as typeof fetch;
});

afterEach(() => {
    globalThis.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.HOME;
    else process.env.HOME = originalHome;
    if (originalPiDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
    else process.env.PI_CODING_AGENT_DIR = originalPiDir;
    if (originalConfigHome === undefined) delete process.env.XDG_CONFIG_HOME;
    else process.env.XDG_CONFIG_HOME = originalConfigHome;

    for (const path of tempRoots.splice(0)) {
        rmSync(path, { recursive: true, force: true });
    }
});

describe("Pi settings rollback", () => {
    it("removes only the package entry added by setup", () => {
        const root = makeTempRoot();
        const settingsPath = join(root, "settings.json");
        writeFileSync(
            settingsPath,
            JSON.stringify({ packages: ["unrelated-package", "npm:@eidnara/pi"] }),
        );

        expect(removePiSettingsPackage(settingsPath)).toBe(true);
        expect(parseJsonc(readFileSync(settingsPath, "utf-8"))).toEqual({
            packages: ["unrelated-package"],
        });
        expect(removePiSettingsPackage(settingsPath)).toBe(false);
    });

    it("restores an absent packages field when setup added the only entry", () => {
        const root = makeTempRoot();
        const settingsPath = join(root, "settings.json");
        writeFileSync(settingsPath, "{}");

        expect(writePiSettingsPackage(settingsPath)).toBe(true);
        expect(removePiSettingsPackage(settingsPath, "npm:@eidnara/pi", true)).toBe(true);
        expect(parseJsonc(readFileSync(settingsPath, "utf-8"))).toEqual({});
    });

    it("refuses to overwrite a non-array packages value instead of discarding it", () => {
        const root = makeTempRoot();
        const settingsPath = join(root, "settings.json");
        const original = JSON.stringify({ packages: { legacy: true } });
        writeFileSync(settingsPath, original);

        expect(() => writePiSettingsPackage(settingsPath)).toThrow(/expected an array/);
        expect(readFileSync(settingsPath, "utf-8")).toBe(original);
    });

    it("does not rewrite settings.json when the package is already registered", () => {
        const root = makeTempRoot();
        const settingsPath = join(root, "settings.json");
        const original = `{\n\t"packages": [ /* mine */ "npm:@eidnara/pi" ],\n\t"other": 1 // keep\n}\n`;
        writeFileSync(settingsPath, original);

        expect(writePiSettingsPackage(settingsPath)).toBe(false);
        expect(readFileSync(settingsPath, "utf-8")).toBe(original);
    });

    it("keeps comments on the remaining package entries when removing the Eidnara entry", () => {
        const root = makeTempRoot();
        const settingsPath = join(root, "settings.json");
        writeFileSync(
            settingsPath,
            `{\n  "packages": [\n    // first\n    "npm:one",\n    /* second */ "npm:two",\n    "npm:@eidnara/pi"\n  ]\n}\n`,
        );

        expect(removePiSettingsPackage(settingsPath)).toBe(true);
        const written = readFileSync(settingsPath, "utf-8");
        expect(written).toContain("// first");
        expect(written).toContain("/* second */");
        expect(parseJsonc(written)).toEqual({ packages: ["npm:one", "npm:two"] });
    });
});

describe("runSetup", () => {
    it("aborts before writing when an existing target is malformed", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const settingsPath = join(agentDir, "settings.json");
        const malformed = `{\n  "packages": [\n`;
        writeFileSync(settingsPath, malformed);

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => join(root, ".config", "eidnara", "eidnara.jsonc"),
                getPiUserExtensionsPath: () => settingsPath,
            },
        };
        // The single confirmation is configurePi=true, which makes settings.json a write target.
        const prompts = new MockPrompts({ confirms: [true] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(1);
        expect(readFileSync(settingsPath, "utf-8")).toBe(malformed);
        expect(existsSync(env.paths.getPiUserConfigPath())).toBe(false);
        expect(prompts.messages.join("\n")).toContain(
            `Refusing to overwrite unparseable config ${settingsPath} at line 3, column 1`,
        );
    });

    it("writes the shared config when registration is skipped and only settings.json is malformed", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const settingsPath = join(agentDir, "settings.json");
        const malformed = `{\n  "packages": [\n`;
        writeFileSync(settingsPath, malformed);
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => settingsPath,
            },
        };
        // The confirmations are configurePi=false and sidekickEnabled=false.
        const prompts = new MockPrompts({ confirms: [false, false] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        expect(readFileSync(settingsPath, "utf-8")).toBe(malformed);
        const config = parseJsonc(readFileSync(configPath, "utf-8")) as {
            historian?: { model?: string };
        };
        expect(config.historian?.model).toBe("anthropic/claude-haiku-4-5");
        expect(prompts.messages.join("\n")).toContain("Skipped Pi package registration.");
    });

    it("keeps JSONC comments in an existing eidnara config when rewriting it", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(
            configPath,
            [
                "{",
                "  // top-level note",
                '  "historian": {',
                "    // inside historian",
                '    "model": "anthropic/claude-sonnet-4-6"',
                "  },",
                "  /* compaction stays off */",
                '  "compaction": { "enabled": false }',
                "}",
                "",
            ].join("\n"),
        );

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        // The confirmations are configurePi=true and sidekickEnabled=false.
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        const written = readFileSync(configPath, "utf-8");
        expect(written).toContain("// top-level note");
        expect(written).toContain("// inside historian");
        expect(written).toContain("/* compaction stays off */");
        const config = parseJsonc(written) as {
            historian?: { model?: string };
            compaction?: { enabled?: boolean };
        };
        expect(config.historian?.model).toBe("anthropic/claude-haiku-4-5");
        expect(config.compaction?.enabled).toBe(false);
    });

    it("skips the native-settings rollback when the plugin registration cannot be undone", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        // A regular file at the config's parent path fails the write but not the pre-write validation.
        writeFileSync(join(root, "not-a-dir"), "");
        const configPath = join(root, "not-a-dir", "eidnara.jsonc");
        const settingsPath = join(agentDir, "settings.json");

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => settingsPath,
            },
        };
        const calls: string[] = [];
        const host: PiCompatibleSetupHost = {
            displayName: "Fake",
            cliName: "fake",
            packageSource: "npm:fake",
            ensurePluginEntry: async () => {
                calls.push("ensurePluginEntry");
                return {
                    ok: true,
                    action: "added",
                    message: "registered",
                    configPath: settingsPath,
                };
            },
            beforeWrite: async () => async () => {
                calls.push("rollbackHost");
            },
            rollbackPluginEntry: async () => {
                calls.push("rollbackPluginEntry");
                throw new Error("plugin undo failed");
            },
        };
        // The confirmations are configureHost=true and sidekickEnabled=false.
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env, host });

        expect(code).toBe(1);
        expect(calls).toEqual(["ensurePluginEntry", "rollbackPluginEntry"]);
        const log = prompts.messages.join("\n");
        expect(log).toContain("error:plugin undo failed");
        expect(log).toContain("two context managers at once");
        expect(log).toContain("outro:Setup stopped — undo the Fake plugin registration by hand");
    });

    it("restores native settings when the plugin registration is undone", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        writeFileSync(join(root, "not-a-dir"), "");
        const configPath = join(root, "not-a-dir", "eidnara.jsonc");
        const settingsPath = join(agentDir, "settings.json");

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => settingsPath,
            },
        };
        const calls: string[] = [];
        const host: PiCompatibleSetupHost = {
            displayName: "Fake",
            cliName: "fake",
            packageSource: "npm:fake",
            ensurePluginEntry: async () => ({
                ok: true,
                action: "added",
                message: "registered",
                configPath: settingsPath,
            }),
            beforeWrite: async () => async () => {
                calls.push("rollbackHost");
            },
            rollbackPluginEntry: async () => {
                calls.push("rollbackPluginEntry");
            },
        };
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env, host });

        expect(code).toBe(1);
        expect(calls).toEqual(["rollbackPluginEntry", "rollbackHost"]);
        expect(prompts.messages.join("\n")).toContain(
            "outro:Setup stopped — rolled back Fake changes.",
        );
    });

    it("reports a partial rollback when restoring native settings fails", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        writeFileSync(join(root, "not-a-dir"), "");
        const configPath = join(root, "not-a-dir", "eidnara.jsonc");

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        const host: PiCompatibleSetupHost = {
            displayName: "Fake",
            cliName: "fake",
            packageSource: "npm:fake",
            ensurePluginEntry: async () => ({
                ok: true,
                action: "already_present",
                message: "present",
                configPath: "unused",
            }),
            beforeWrite: async () => async () => {
                throw new Error(
                    "Could not restore OMP settings; run by hand:\n- omp config set x y",
                );
            },
        };
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env, host });

        expect(code).toBe(1);
        const log = prompts.messages.join("\n");
        expect(log).toContain(
            "error:Could not restore OMP settings; run by hand:\n- omp config set x y",
        );
        expect(log).toContain("outro:Setup stopped — Fake changes were only partly rolled back");
    });

    it("passes the shared config's compaction and memory modes to the host hook", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(
            configPath,
            JSON.stringify({ compaction: { enabled: false }, memory: { enabled: false } }),
        );

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        let seen:
            | { enabled: boolean; compactionEnabled: boolean; memoryEnabled: boolean }
            | undefined;
        const host: PiCompatibleSetupHost = {
            displayName: "Fake",
            cliName: "fake",
            packageSource: "npm:fake",
            ensurePluginEntry: async () => ({
                ok: true,
                action: "already_present",
                message: "present",
                configPath: "unused",
            }),
            beforeWrite: async ({ eidnara }) => {
                seen = eidnara;
                return async () => {};
            },
        };
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env, host });

        expect(code).toBe(0);
        expect(seen).toEqual({ enabled: true, compactionEnabled: false, memoryEnabled: false });
        const config = parseJsonc(readFileSync(configPath, "utf-8")) as {
            compaction?: { enabled?: boolean };
            memory?: { enabled?: boolean };
        };
        expect(config.compaction?.enabled).toBe(false);
        expect(config.memory?.enabled).toBe(false);
    });

    it("turns both modes off and warns when Eidnara is disabled in the shared config", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(configPath, JSON.stringify({ enabled: false }));

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        let seen:
            | { enabled: boolean; compactionEnabled: boolean; memoryEnabled: boolean }
            | undefined;
        const host: PiCompatibleSetupHost = {
            displayName: "Fake",
            cliName: "fake",
            packageSource: "npm:fake",
            ensurePluginEntry: async () => ({
                ok: true,
                action: "already_present",
                message: "present",
                configPath: "unused",
            }),
            beforeWrite: async ({ eidnara }) => {
                seen = eidnara;
                return async () => {};
            },
        };
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env, host });

        expect(code).toBe(0);
        expect(seen).toEqual({ enabled: false, compactionEnabled: false, memoryEnabled: false });
        expect(prompts.messages.join("\n")).toContain(
            "warn:Eidnara is disabled (`enabled: false`)",
        );
        const config = parseJsonc(readFileSync(configPath, "utf-8")) as { enabled?: boolean };
        expect(config.enabled).toBe(false);
    });

    it("warns about a project-level disable without changing the shared-config modes", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");
        const project = join(root, "project");
        mkdirSync(join(project, ".eidnara"), { recursive: true });
        writeFileSync(
            join(project, ".eidnara", "eidnara.jsonc"),
            JSON.stringify({ enabled: false }),
        );
        const originalCwd = process.cwd();
        process.chdir(project);

        try {
            const env: SetupEnvironment = {
                detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
                getPiVersion: () => "0.74.0",
                getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
                paths: {
                    getPiAgentConfigDir: () => agentDir,
                    getPiUserConfigPath: () => configPath,
                    getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
                },
            };
            let seen: { enabled: boolean } | undefined;
            const host: PiCompatibleSetupHost = {
                displayName: "Fake",
                cliName: "fake",
                packageSource: "npm:fake",
                ensurePluginEntry: async () => ({
                    ok: true,
                    action: "already_present",
                    message: "present",
                    configPath: "unused",
                }),
                beforeWrite: async ({ eidnara }) => {
                    seen = eidnara;
                    return async () => {};
                },
            };
            const prompts = new MockPrompts({ confirms: [true, false] });

            const code = await runSetup({ prompts, env, host });

            expect(code).toBe(0);
            expect(seen?.enabled).toBe(true);
            expect(prompts.messages.join("\n")).toContain(
                `warn:Project config ${join(project, ".eidnara", "eidnara.jsonc")} overrides enabled: false;`,
            );
        } finally {
            process.chdir(originalCwd);
        }
    });

    it("warns when the project config re-enables modes the shared config turned off", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(configPath, JSON.stringify({ enabled: false, memory: { enabled: false } }));
        const project = join(root, "project");
        mkdirSync(join(project, ".eidnara"), { recursive: true });
        writeFileSync(
            join(project, ".eidnara", "eidnara.jsonc"),
            JSON.stringify({ enabled: true, memory: { enabled: true } }),
        );
        const originalCwd = process.cwd();
        process.chdir(project);

        try {
            const env: SetupEnvironment = {
                detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
                getPiVersion: () => "0.74.0",
                getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
                paths: {
                    getPiAgentConfigDir: () => agentDir,
                    getPiUserConfigPath: () => configPath,
                    getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
                },
            };
            let seen: { enabled: boolean; memoryEnabled: boolean } | undefined;
            const host: PiCompatibleSetupHost = {
                displayName: "Fake",
                cliName: "fake",
                packageSource: "npm:fake",
                ensurePluginEntry: async () => ({
                    ok: true,
                    action: "already_present",
                    message: "present",
                    configPath: "unused",
                }),
                beforeWrite: async ({ eidnara }) => {
                    seen = eidnara;
                    return async () => {};
                },
            };
            const prompts = new MockPrompts({ confirms: [true, false] });

            const code = await runSetup({ prompts, env, host });

            expect(code).toBe(0);
            expect(seen).toEqual({
                enabled: false,
                compactionEnabled: false,
                memoryEnabled: false,
            });
            expect(prompts.messages.join("\n")).toContain(
                "overrides enabled: true, memory.enabled: true;",
            );
        } finally {
            process.chdir(originalCwd);
        }
    });

    it("persists a sidekick thinking level for GitHub Copilot models", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["github-copilot/gpt-5.4"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => configPath,
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        const prompts = new MockPrompts({ confirms: [true, true] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        const config = parseJsonc(readFileSync(configPath, "utf-8")) as {
            historian?: { model?: string; thinking_level?: string };
            sidekick?: { model?: string; thinking_level?: string; disable?: boolean };
        };
        expect(config.historian?.thinking_level).toBe("medium");
        expect(config.sidekick?.model).toBe("github-copilot/gpt-5.4");
        expect(config.sidekick?.thinking_level).toBe("medium");
        expect(config.sidekick?.disable).toBeUndefined();
        expect(prompts.messages.join("\n")).toContain(
            "Sidekick: github-copilot/gpt-5.4 (thinking: medium)",
        );
    });

    it("updates an existing eidnara.json instead of shadowing it with a new eidnara.jsonc", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(configDir, { recursive: true });
        writeFileSync(join(configDir, "eidnara.json"), JSON.stringify({ language: "de" }));

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: getSharedUserConfigPath,
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        expect(existsSync(join(configDir, "eidnara.jsonc"))).toBe(false);
        const config = parseJsonc(readFileSync(join(configDir, "eidnara.json"), "utf-8")) as {
            language?: string;
            historian?: { model?: string };
        };
        expect(config.language).toBe("de");
        expect(config.historian?.model).toBe("anthropic/claude-haiku-4-5");
    });

    it("round-trips mixed string and object package entries when adding Eidnara", () => {
        const root = makeTempRoot();
        const settingsPath = join(root, "settings.json");
        mkdirSync(root, { recursive: true });
        writeFileSync(
            settingsPath,
            JSON.stringify({
                packages: ["npm:one", { name: "two", version: "2.0.0" }],
            }),
        );

        const added = writePiSettingsPackage(settingsPath);
        const updated = parseJsonc(readFileSync(settingsPath, "utf-8")) as {
            packages?: unknown[];
        };

        expect(added).toBe(true);
        expect(updated.packages).toEqual([
            "npm:one",
            { name: "two", version: "2.0.0" },
            "npm:@eidnara/pi",
        ]);
    });

    it("writes Pi settings and eidnara config with mocked prompts", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            getAvailableModels: () => [
                "anthropic/claude-haiku-4-5",
                "anthropic/claude-sonnet-4-6",
                "github-copilot/gemini-3-flash-preview",
            ],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => join(root, ".config", "eidnara", "eidnara.jsonc"),
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        // MockPrompts consumes confirmations as configurePi=true and sidekickEnabled=false.
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        expect(fetchCalls).toEqual([]);
        const settingsPath = join(agentDir, "settings.json");
        const configPath = join(root, ".config", "eidnara", "eidnara.jsonc");
        expect(existsSync(settingsPath)).toBe(true);
        expect(existsSync(configPath)).toBe(true);

        const settings = parseJsonc(readFileSync(settingsPath, "utf-8")) as {
            packages?: string[];
        };
        expect(settings.packages).toContain("npm:@eidnara/pi");

        const config = parseJsonc(readFileSync(configPath, "utf-8")) as {
            historian?: { model?: string; thinking_level?: string };
            sidekick?: { enabled?: boolean; disable?: boolean };
        };
        // The picker shows the full model list sorted alphabetically.
        // The mock selects "anthropic/claude-haiku-4-5" for the historian.
        expect(config.historian?.model).toBe("anthropic/claude-haiku-4-5");
        expect(config.historian?.thinking_level).toBeUndefined();
        expect(config.sidekick?.disable).toBe(true);
        expect(config.sidekick).not.toHaveProperty("enabled");
    });

    it("prompts for thinking_level when historian model is github-copilot", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        mkdirSync(agentDir, { recursive: true });

        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: join(root, "bin", "pi"), source: "path" }),
            getPiVersion: () => "0.74.0",
            // The single available model makes the picker deterministic.
            getAvailableModels: () => ["github-copilot/gpt-5.4"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => join(root, ".config", "eidnara", "eidnara.jsonc"),
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        // selectOne picks the recommended option ("medium" for thinking_level)
        // MockPrompts consumes confirmations as configurePi=true and sidekickEnabled=false.
        const prompts = new MockPrompts({ confirms: [true, false] });

        const code = await runSetup({ prompts, env });
        expect(code).toBe(0);

        const config = parseJsonc(
            readFileSync(join(root, ".config", "eidnara", "eidnara.jsonc"), "utf-8"),
        ) as {
            historian?: { model?: string; thinking_level?: string };
        };
        // The setup wizard must set thinking_level for github-copilot models.
        expect(config.historian?.model).toBe("github-copilot/gpt-5.4");
        expect(config.historian?.thinking_level).toBe("medium");
    });

    it("exits gracefully without writing files when Pi is missing", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        const env: SetupEnvironment = {
            detectPiBinary: () => null,
            getPiVersion: () => null,
            getAvailableModels: () => [],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => join(root, ".config", "eidnara", "eidnara.jsonc"),
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        const prompts = new MockPrompts({ confirms: [] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(1);
        expect(existsSync(agentDir)).toBe(false);
        expect(prompts.messages.join("\n")).toContain("Pi not found");
    });

    it("warns and exits when Pi version is below 0.74.0 and user declines", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: "/usr/local/bin/pi", source: "path" }),
            getPiVersion: () => "0.69.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => join(root, ".config", "eidnara", "eidnara.jsonc"),
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        // `false` declines the continue-anyway prompt.
        const prompts = new MockPrompts({ confirms: [false] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        const log = prompts.messages.join("\n");
        expect(log).toContain("Pi 0.69.0 is older than the required 0.74.0");
        expect(log).toContain("outro:Setup cancelled");
        expect(existsSync(join(root, ".config", "eidnara", "eidnara.jsonc"))).toBe(false);
        expect(existsSync(join(agentDir, "settings.json"))).toBe(false);
    });

    it("continues setup when Pi version is below 0.74.0 and user opts in", async () => {
        const root = makeTempRoot();
        const agentDir = join(root, ".pi", "agent");
        setConfigEnv(root, agentDir);
        const env: SetupEnvironment = {
            detectPiBinary: () => ({ path: "/usr/local/bin/pi", source: "path" }),
            getPiVersion: () => "0.69.0",
            getAvailableModels: () => ["anthropic/claude-haiku-4-5"],
            paths: {
                getPiAgentConfigDir: () => agentDir,
                getPiUserConfigPath: () => join(root, ".config", "eidnara", "eidnara.jsonc"),
                getPiUserExtensionsPath: () => join(agentDir, "settings.json"),
            },
        };
        // The confirmations are continue-anyway=true, configurePi=true, and sidekickEnabled=false.
        const prompts = new MockPrompts({ confirms: [true, true, false] });

        const code = await runSetup({ prompts, env });

        expect(code).toBe(0);
        expect(existsSync(join(root, ".config", "eidnara", "eidnara.jsonc"))).toBe(true);
        expect(prompts.messages.join("\n")).toContain(
            "Pi 0.69.0 is older than the required 0.74.0",
        );
    });
});
