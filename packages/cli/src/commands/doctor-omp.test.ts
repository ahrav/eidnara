import { afterEach, describe, expect, it } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { PromptIO, PromptSpinner, SelectOption } from "../lib/prompts";
import { runDoctor } from "./doctor-omp";

class MockPrompts implements PromptIO {
    readonly messages: string[] = [];
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
    outro(): void {}
    note(): void {}
    spinner(): PromptSpinner {
        return { start: () => {}, stop: () => {}, message: () => {} };
    }
    async confirm(): Promise<boolean> {
        return false;
    }
    async text(): Promise<string> {
        return "test";
    }
    async selectOne(_message: string, options: SelectOption[]): Promise<string> {
        return options[0]?.value ?? "";
    }
    async selectMany(_message: string, options: SelectOption[]): Promise<string[]> {
        return options.map((option) => option.value);
    }
    async selectAutocomplete(_message: string, options: SelectOption[]): Promise<string> {
        return options[0]?.value ?? "";
    }
}

const roots: string[] = [];
const original = {
    HOME: process.env.HOME,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
    PI_CODING_AGENT_DIR: process.env.PI_CODING_AGENT_DIR,
    PI_CONFIG_FILES: process.env.PI_CONFIG_FILES,
};

afterEach(() => {
    for (const [key, value] of Object.entries(original)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("OMP doctor", () => {
    it("accepts a healthy OMP installation", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    {
                        name: "@eidnara/pi",
                        version: "0.33.0",
                        enabled: true,
                        path: pluginDir,
                    },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: `${agentDir}/./`, stderr: "" }),
            },
        });

        expect(code).toBe(0);
        expect(prompts.messages.join("\n")).toContain("OMP 17.1.7 detected");
        expect(prompts.messages.join("\n")).toContain("FAIL 0");
    });

    it("repairs a missing config when it is the only health finding", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-config-only-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    {
                        name: "@eidnara/pi",
                        version: "0.35.1",
                        enabled: true,
                        path: pluginDir,
                    },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(0);
        expect(existsSync(join(root, ".config", "eidnara", "eidnara.jsonc"))).toBe(true);
        expect(prompts.messages.join("\n")).toContain("Wrote default Eidnara config");
    });

    it("writes an independent default config even when OMP is missing", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-no-bin-"));
        roots.push(root);
        process.env.HOME = root;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        delete process.env.PI_CODING_AGENT_DIR;
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: { detectOmpBinary: () => null },
        });

        expect(code).toBe(1);
        expect(existsSync(join(root, ".config", "eidnara", "eidnara.jsonc"))).toBe(true);
        expect(prompts.messages.join("\n")).toContain("Wrote default Eidnara config");
    });

    it("does not repair global settings through a project config override", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-project-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        mkdirSync(join(root, ".omp"), { recursive: true });
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(join(root, ".omp", "config.yml"), "compaction:\n  enabled: true\n");
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(root, ".config", "eidnara", "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    {
                        name: "@eidnara/pi",
                        version: "0.33.0",
                        enabled: true,
                        path: pluginDir,
                    },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "mnemopi") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(1);
        expect(calls.some((args) => args[0] === "config" && args[1] === "set")).toBe(false);
        expect(prompts.messages.join("\n")).toContain("automatic global repair is disabled");
    });

    it("rejects an array at the Eidnara config root", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-array-"));
        roots.push(root);
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(join(root, ".config", "eidnara", "eidnara.jsonc"), "[]\n");
        process.env.HOME = root;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            prompts,
            deps: { detectOmpBinary: () => null },
        });

        expect(code).toBe(1);
        expect(prompts.messages.join("\n")).toContain("Invalid Eidnara user config");
    });

    it("does not write a default eidnara.jsonc over an existing eidnara.json in --force mode", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-json-"));
        roots.push(root);
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(configDir, { recursive: true });
        writeFileSync(join(configDir, "eidnara.json"), JSON.stringify({ enabled: false }));
        process.env.HOME = root;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: { detectOmpBinary: () => null },
        });

        expect(code).toBe(1);
        expect(existsSync(join(configDir, "eidnara.jsonc"))).toBe(false);
        const output = prompts.messages.join("\n");
        expect(output).toContain("Eidnara user config parses: eidnara.json");
        expect(output).not.toContain("No Eidnara user config");
        expect(output).not.toContain("Wrote default Eidnara config");
    });

    it("fails when the project config is malformed even though the loader recovers", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-project-bad-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        mkdirSync(join(root, ".eidnara"), { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        writeFileSync(join(root, ".eidnara", "eidnara.jsonc"), "{ nope\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    {
                        name: "@eidnara/pi",
                        version: "0.33.0",
                        enabled: true,
                        path: pluginDir,
                    },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(1);
        expect(prompts.messages.join("\n")).toContain("Invalid Eidnara project config");
    });

    it("reports an uninstalled plugin with the install command and does not try to repair it", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-uninstalled-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(join(root, ".config", "eidnara", "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(1);
        const output = prompts.messages.join("\n");
        expect(output).toContain("is not installed in OMP. Run `omp plugin install @eidnara/pi`");
        expect(output).not.toContain("Failed to configure OMP");
        expect(calls.some((args) => args[0] === "plugin")).toBe(false);
    });

    it("fails when the installed plugin declares an empty extension manifest", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-empty-manifest-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(join(root, ".config", "eidnara"), { recursive: true });
        writeFileSync(join(pluginDir, "package.json"), JSON.stringify({ omp: { extensions: [] } }));
        writeFileSync(join(root, ".config", "eidnara", "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: true, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(1);
        expect(prompts.messages.join("\n")).toContain(
            "Installed plugin has no OMP/Pi extension manifest",
        );
    });

    it("caps the issue body at GitHub's limit before writing it", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-cap-"));
        roots.push(root);
        process.env.HOME = root;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        class LongPrompts extends MockPrompts {
            override async text(): Promise<string> {
                return "x".repeat(100_000);
            }
        }
        const prompts = new LongPrompts();

        const code = await runDoctor({
            cwd: root,
            issue: true,
            prompts,
            deps: {
                detectOmpBinary: () => null,
                now: () => new Date("2026-04-28T12:34:56Z"),
                execFileSync: (() => {
                    throw new Error("gh unavailable");
                }) as never,
            },
        });

        expect(code).toBe(0);
        const reportPath = join(root, "eidnara-omp-issue-20260428T123456Z.md");
        expect(existsSync(reportPath)).toBe(true);
        expect(statSync(reportPath).size).toBeLessThanOrEqual(60_000 + 1);
    });

    it("keeps OMP compaction and memory on when the effective Eidnara config turns them off", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-modes-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(
            join(configDir, "eidnara.jsonc"),
            JSON.stringify({ compaction: { enabled: false }, memory: { enabled: false } }),
        );
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: true, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "mnemopi") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(0);
        const output = prompts.messages.join("\n");
        expect(output).toContain(
            "OMP native compaction stays enabled because Eidnara compaction is off",
        );
        expect(output).toContain(
            "OMP memory.backend=mnemopi stays enabled because Eidnara memory is off",
        );
        expect(output).not.toContain("conflicts with Eidnara");
        expect(calls.some((args) => args[0] === "config" && args[1] === "set")).toBe(false);
    });

    it("treats enabled: false as turning both Eidnara managers off", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-disabled-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), JSON.stringify({ enabled: false }));
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: true, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "mnemopi") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(0);
        expect(calls.some((args) => args[0] === "config" && args[1] === "set")).toBe(false);
    });

    it("does not turn off OMP compaction or memory while the plugin is not enabled", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-gate-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                // The plugin is absent, so nothing could replace the native managers.
                listOmpPlugins: () => [],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "mnemopi") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(1);
        expect(calls.some((args) => args[0] === "config" && args[1] === "set")).toBe(false);
        expect(prompts.messages.join("\n")).toContain(
            "Leaving OMP native compaction and memory on: @eidnara/pi is not enabled in OMP",
        );
    });

    it("does not turn off OMP managers when the enabled plugin has no verifiable manifest", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-gate-manifest-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                // Enabled, but OMP reports no install path, so the manifest cannot be read.
                listOmpPlugins: () => [{ name: "@eidnara/pi", version: "0.33.0", enabled: true }],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "mnemopi") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(1);
        expect(calls.some((args) => args[0] === "config" && args[1] === "set")).toBe(false);
    });

    it("reports historian dumps for recent OMP sessions", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-dumps-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        const project = join(root, "project");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        // One OMP session whose header names the project; the project holds one dump.
        const slugDir = join(agentDir, "sessions", "--tmp-omp-project--");
        mkdirSync(slugDir, { recursive: true });
        writeFileSync(
            join(slugDir, "2026-07-07T12-00-00-000Z_omp1.jsonl"),
            `${JSON.stringify({ type: "session", version: 3, id: "omp1", cwd: project })}\n`,
        );
        const dumpDir = join(project, ".eidnara", "context", "historian");
        mkdirSync(dumpDir, { recursive: true });
        writeFileSync(join(dumpDir, "dump-001.xml"), "<compartments></compartments>\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: true, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(0);
        const output = prompts.messages.join("\n");
        expect(output).toContain("Historian debug dumps: 1 file(s) across 1 project(s)");
        expect(output).toContain(`[${project}] 1 file(s)`);
        expect(output).toContain("dump-001.xml");
    });

    it("treats an OMP prerelease of the minimum as older than the minimum", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-prerelease-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7-beta.1",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: true, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(1);
        expect(prompts.messages.join("\n")).toContain(
            "OMP 17.1.7-beta.1 is older than tested minimum 17.1.7",
        );
    });

    it("does not enable @eidnara/pi under --force when its install has no extension manifest", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-no-manifest-enable-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();
        let enableCalls = 0;

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                ensurePluginEntry: async () => {
                    enableCalls += 1;
                    return { ok: true, action: "updated", message: "Enabled", configPath: "" };
                },
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: false, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(1);
        expect(enableCalls).toBe(0);
        expect(prompts.messages.join("\n")).toContain(
            `Leaving @eidnara/pi disabled: its install at ${pluginDir} has no verifiable OMP/Pi extension manifest`,
        );
    });

    it("disables @eidnara/pi again when a native-manager repair fails after enabling it", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-rollback-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();
        let enabled = false;
        const calls: string[][] = [];

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.1.7",
                ensurePluginEntry: async () => {
                    enabled = true;
                    return { ok: true, action: "updated", message: "Enabled", configPath: "" };
                },
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "off") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    if (args[0] === "config" && args[1] === "set") {
                        return { ok: false, stdout: "", stderr: "settings file is read-only" };
                    }
                    if (args[0] === "plugin" && args[1] === "disable") enabled = false;
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(1);
        expect(calls).toContainEqual(["plugin", "disable", "@eidnara/pi"]);
        const output = prompts.messages.join("\n");
        expect(output).toContain("settings file is read-only");
        expect(output).toContain(
            "Disabled @eidnara/pi again: a native manager could not be turned off",
        );
    });

    it("does not enable @eidnara/pi under --force when OMP is below the minimum", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-old-host-enable-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.0.0",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: false, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? false : "off") as never,
                runOmpCommand: () => ({ ok: true, stdout: agentDir, stderr: "" }),
            },
        });

        expect(code).toBe(1);
        const output = prompts.messages.join("\n");
        expect(output).toContain(
            "Leaving @eidnara/pi and OMP native compaction and memory as they are: this OMP is missing a version or older than 17.1.7",
        );
        // The adapter reports a missing binary when it runs; its absence shows it never ran.
        expect(output).not.toContain("OMP binary not found");
        expect(output).not.toContain("Enabled @eidnara/pi");
    });

    it("does not turn off OMP managers under --force when OMP is below the minimum", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-old-host-"));
        roots.push(root);
        const agentDir = join(root, ".omp", "agent");
        const pluginDir = join(root, "plugin");
        const configDir = join(root, ".config", "eidnara");
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(pluginDir, { recursive: true });
        mkdirSync(configDir, { recursive: true });
        writeFileSync(
            join(pluginDir, "package.json"),
            JSON.stringify({ omp: { extensions: ["./dist/index.js"] } }),
        );
        writeFileSync(join(configDir, "eidnara.jsonc"), "{}\n");
        process.env.HOME = root;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const calls: string[][] = [];
        const prompts = new MockPrompts();

        const code = await runDoctor({
            cwd: root,
            force: true,
            prompts,
            deps: {
                detectOmpBinary: () => ({ path: "/fake/omp", source: "path" }),
                getOmpVersion: () => "17.0.0",
                listOmpPlugins: () => [
                    { name: "@eidnara/pi", version: "0.33.0", enabled: true, path: pluginDir },
                ],
                getOmpSetting: ((_path: string, key: string) =>
                    key === "compaction.enabled" ? true : "mnemopi") as never,
                runOmpCommand: (_path, args) => {
                    calls.push(args);
                    return { ok: true, stdout: agentDir, stderr: "" };
                },
            },
        });

        expect(code).toBe(1);
        expect(calls.some((args) => args[0] === "config" && args[1] === "set")).toBe(false);
        expect(prompts.messages.join("\n")).toContain(
            "Leaving @eidnara/pi and OMP native compaction and memory as they are: this OMP is missing a version or older than 17.1.7",
        );
    });

    it("sanitizes the issue title before passing it to gh issue create", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-doctor-title-"));
        roots.push(root);
        process.env.HOME = root;
        process.env.XDG_CONFIG_HOME = join(root, ".config");
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        const ghCalls: string[][] = [];
        class SubmittingPrompts extends MockPrompts {
            override async confirm(): Promise<boolean> {
                return true;
            }
            override async text(): Promise<string> {
                return "Crash in /home/alice/private token=abc123";
            }
        }
        const prompts = new SubmittingPrompts();

        const code = await runDoctor({
            cwd: root,
            issue: true,
            prompts,
            deps: {
                detectOmpBinary: () => null,
                execFileSync: (() => "") as never,
                spawnSync: ((_command: string, args: string[]) => {
                    ghCalls.push(args);
                    return { status: 0, stdout: "https://github.com/x/1", stderr: "" };
                }) as never,
            },
        });

        expect(code).toBe(0);
        expect(ghCalls).toHaveLength(1);
        const args = ghCalls[0] ?? [];
        const titleArg = args[args.indexOf("--title") + 1] ?? "";
        expect(titleArg).toBe("[omp] Crash in /home/<USER>/private token=<REDACTED:token>");
    });
});
