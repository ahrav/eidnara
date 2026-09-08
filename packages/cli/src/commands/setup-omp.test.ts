import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import type { PromptIO, PromptSpinner, SelectOption } from "../lib/prompts";
import { __test } from "./setup-omp";

class MockPrompts implements PromptIO {
    readonly messages: string[] = [];
    constructor(private readonly confirms: boolean[]) {}
    readonly log = {
        info: (message: string) => this.messages.push(`info:${message}`),
        success: (message: string) => this.messages.push(`success:${message}`),
        warn: (message: string) => this.messages.push(`warn:${message}`),
        error: (message: string) => this.messages.push(`error:${message}`),
        message: (message: string) => this.messages.push(`message:${message}`),
        step: (message: string) => this.messages.push(`step:${message}`),
    };
    intro(): void {}
    outro(): void {}
    note(): void {}
    spinner(): PromptSpinner {
        return { start: () => {}, stop: () => {}, message: () => {} };
    }
    async confirm(): Promise<boolean> {
        const value = this.confirms.shift();
        if (value === undefined) throw new Error("missing confirm response");
        return value;
    }
    async text(): Promise<string> {
        return "";
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
    PATH: process.env.PATH,
    HOME: process.env.HOME,
    PI_CONFIG_FILES: process.env.PI_CONFIG_FILES,
};
const originalFetch = globalThis.fetch;
const fetchCalls: unknown[] = [];

beforeEach(() => {
    fetchCalls.length = 0;
    globalThis.fetch = ((...args: unknown[]) => {
        fetchCalls.push(args);
        throw new Error("network call");
    }) as unknown as typeof fetch;
});

afterEach(() => {
    globalThis.fetch = originalFetch;
    for (const [key, value] of Object.entries(original)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function makeFakeOmp(
    options: {
        failMemorySet?: boolean;
        failPluginCommands?: boolean;
        failPluginList?: boolean;
        failConfigSet?: { key: string; value: string };
    } = {},
): {
    root: string;
    binary: string;
    state: string;
    pluginLog: string;
} {
    const root = mkdtempSync(join(tmpdir(), "eidnara-omp-setup-"));
    roots.push(root);
    const state = join(root, "state.json");
    const pluginLog = join(root, "plugin-commands.log");
    const binary = join(root, "omp");
    writeFileSync(state, JSON.stringify({ compaction: true, memory: "mnemopi" }));
    writeFileSync(
        binary,
        `#!/usr/bin/env node
const fs = require("fs");
const statePath = ${JSON.stringify(state)};
const pluginLogPath = ${JSON.stringify(pluginLog)};
const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
const failMemorySet = ${JSON.stringify(options.failMemorySet === true)};
const failPluginCommands = ${JSON.stringify(options.failPluginCommands === true)};
const failPluginList = ${JSON.stringify(options.failPluginList === true)};
const failConfigSet = ${JSON.stringify(options.failConfigSet ?? null)};
const args = process.argv.slice(2);
if (args[0] === "config" && args[1] === "get") {
  const value = args[2] === "compaction.enabled" ? state.compaction : state.memory;
  process.stdout.write(JSON.stringify({ value }));
} else if (args[0] === "config" && args[1] === "set") {
  if (failConfigSet && args[2] === failConfigSet.key && args[3] === failConfigSet.value) {
    process.stderr.write("config set " + args[2] + " refused");
    process.exit(1);
  }
  if (args[2] === "compaction.enabled") state.compaction = args[3] === "true";
  else {
    if (failMemorySet) {
      process.stderr.write("memory set failed");
      process.exit(1);
    }
    state.memory = args[3];
  }
  fs.writeFileSync(statePath, JSON.stringify(state));
} else if (args[0] === "plugin" && args[1] === "list") {
  if (failPluginList) {
    process.stderr.write("plugin list failed");
    process.exit(1);
  }
  process.stdout.write(JSON.stringify({ npm: [], marketplace: [] }));
} else if (args[0] === "plugin") {
  fs.appendFileSync(pluginLogPath, args.join(" ") + "\\n");
  if (failPluginCommands) {
    process.stderr.write("plugin command failed");
    process.exit(1);
  }
}
`,
    );
    chmodSync(binary, 0o755);
    process.env.PATH = [root, original.PATH].filter(Boolean).join(delimiter);
    process.env.HOME = root;
    return { root, binary, state, pluginLog };
}

describe("OMP setup transaction", () => {
    it("refuses project/overlay config even when effective settings already match", async () => {
        const { binary, state } = makeFakeOmp();
        writeFileSync(state, JSON.stringify({ compaction: false, memory: "off" }));
        const cwd = mkdtempSync(join(tmpdir(), "eidnara-omp-project-"));
        roots.push(cwd);
        mkdirSync(join(cwd, ".omp"), { recursive: true });
        writeFileSync(
            join(cwd, ".omp", "config.yml"),
            "compaction:\n  enabled: false\nmemory:\n  backend: off\n",
        );
        const prompts = new MockPrompts([]);

        const result = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: true,
        });

        expect(result).toBe(false);
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: false,
            memory: "off",
        });
        expect(prompts.messages.join("\n")).toContain("refusing to mutate the global config");
        expect(prompts.messages.join("\n")).toContain(join(cwd, ".omp", "config.yml"));
    });

    it("fails closed when registration is skipped and the plugin probe fails", async () => {
        const { root, binary, state } = makeFakeOmp({ failPluginList: true });
        const prompts = new MockPrompts([]);

        const result = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd: root,
            prompts,
            dryRun: false,
            configureHost: false,
            eidnaraCompactionEnabled: true,
        });

        expect(result).toBe(false);
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
        expect(prompts.messages.join("\n")).toContain("Could not list OMP plugins");
    });

    it("rollback throws with the manual commands when a restoration fails", async () => {
        const { root, binary, state } = makeFakeOmp({
            failConfigSet: { key: "compaction.enabled", value: "true" },
        });
        const prompts = new MockPrompts([true, true]);
        const rollback = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd: root,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: true,
        });
        expect(typeof rollback).toBe("function");
        if (typeof rollback !== "function") return;

        await expect(rollback()).rejects.toThrow(
            /Could not restore OMP settings[\s\S]*omp config set compaction\.enabled true/,
        );
        // Memory is restored first (reverse order); the compaction restore is the one that failed.
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: false,
            memory: "mnemopi",
        });
        expect(prompts.messages.join("\n")).toContain("Restored OMP memory.backend=mnemopi");
    });

    it("leaves OMP native compaction on when Eidnara compaction is off", async () => {
        const { root, binary, state } = makeFakeOmp();
        // The single confirmation disables the memory backend; no compaction prompt is issued.
        const prompts = new MockPrompts([true]);
        const rollback = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd: root,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: false,
        });
        expect(typeof rollback).toBe("function");
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "off",
        });
        expect(prompts.messages.join("\n")).toContain("leaving OMP native compaction enabled");

        if (typeof rollback === "function") await rollback();
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
    });

    it("reports a failed plugin rollback instead of discarding the result", async () => {
        const { pluginLog } = makeFakeOmp({ failPluginCommands: true });

        await expect(
            __test.OMP_HOST.rollbackPluginEntry?.({
                ok: true,
                action: "updated",
                message: "enabled",
                configPath: "unused",
            }),
        ).rejects.toThrow(/Could not disable .*plugin command failed.*omp plugin disable/);
        expect(readFileSync(pluginLog, "utf-8").trim()).toBe("plugin disable @eidnara/pi");
    });

    it("uninstalls a plugin that setup installed and leaves an already-present one alone", async () => {
        const { pluginLog } = makeFakeOmp();

        await __test.OMP_HOST.rollbackPluginEntry?.({
            ok: true,
            action: "already_present",
            message: "present",
            configPath: "unused",
        });
        expect(existsSync(pluginLog)).toBe(false);

        await __test.OMP_HOST.rollbackPluginEntry?.({
            ok: true,
            action: "added",
            message: "installed",
            configPath: "unused",
        });
        expect(readFileSync(pluginLog, "utf-8").trim()).toBe("plugin uninstall @eidnara/pi");
    });

    it("restores native settings when a later setup step rolls back", async () => {
        const { root, binary, state } = makeFakeOmp();
        const prompts = new MockPrompts([true, true]);
        const rollback = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd: root,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: true,
        });
        expect(typeof rollback).toBe("function");
        expect(fetchCalls).toEqual([]);
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: false,
            memory: "off",
        });

        if (typeof rollback === "function") await rollback();
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
    });

    it("automatically restores an earlier setting when a later write fails", async () => {
        const { root, binary, state } = makeFakeOmp({ failMemorySet: true });
        const prompts = new MockPrompts([true, true]);

        const result = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd: root,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: true,
        });

        expect(result).toBe(false);
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
        expect(prompts.messages.join("\n")).toContain("memory set failed");
        expect(prompts.messages.join("\n")).toContain("Restored OMP compaction.enabled=true");
    });

    it("does not change native settings when registration is skipped and plugin is absent", async () => {
        const { root, binary, state } = makeFakeOmp();
        const prompts = new MockPrompts([]);
        const rollback = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd: root,
            prompts,
            dryRun: false,
            configureHost: false,
            eidnaraCompactionEnabled: true,
        });
        expect(typeof rollback).toBe("function");
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
    });

    it("refuses global setting writes when a project OMP config is active", async () => {
        const { binary, state } = makeFakeOmp();
        const cwd = mkdtempSync(join(tmpdir(), "eidnara-omp-project-"));
        roots.push(cwd);
        mkdirSync(join(cwd, ".omp"), { recursive: true });
        writeFileSync(join(cwd, ".omp", "config.yml"), "compaction:\n  enabled: true\n");
        const prompts = new MockPrompts([true, true]);

        const result = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: true,
        });

        expect(result).toBe(false);
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
        expect(prompts.messages.join("\n")).toContain("refusing to mutate the global config");
    });

    it("refuses global setting writes when PI_CONFIG_FILES overlays are active", async () => {
        const { binary, state } = makeFakeOmp();
        const cwd = mkdtempSync(join(tmpdir(), "eidnara-omp-overlay-"));
        roots.push(cwd);
        process.env.PI_CONFIG_FILES = "settings/omp.yml";
        const prompts = new MockPrompts([true, true]);

        const result = await __test.OMP_HOST.beforeWrite?.({
            binaryPath: binary,
            cwd,
            prompts,
            dryRun: false,
            configureHost: true,
            eidnaraCompactionEnabled: true,
        });

        expect(result).toBe(false);
        expect(JSON.parse(readFileSync(state, "utf-8"))).toEqual({
            compaction: true,
            memory: "mnemopi",
        });
        expect(prompts.messages.join("\n")).toContain(join(cwd, "settings", "omp.yml"));
    });
});
