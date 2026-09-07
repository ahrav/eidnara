/// <reference types="bun-types" />

import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parse as parseJsonc } from "comment-json";
import { detectConflicts } from "./conflict-detector";
import { fixConflicts } from "./conflict-fixer";

const noOmoConflicts = {
    omoPreemptiveCompaction: false,
    omoContextWindowMonitor: false,
    omoAnthropicRecovery: false,
};

describe("fixConflicts", () => {
    let root: string;
    let projectDir: string;
    let userConfigDir: string;
    let homeDir: string;
    let originalEnv: Record<string, string | undefined>;

    beforeEach(() => {
        root = mkdtempSync(join(tmpdir(), "eidnara-conflict-fixer-"));
        projectDir = join(root, "project");
        userConfigDir = join(root, "user-config", "opencode");
        homeDir = join(root, "home");
        mkdirSync(projectDir, { recursive: true });
        mkdirSync(userConfigDir, { recursive: true });
        mkdirSync(homeDir, { recursive: true });
        originalEnv = {
            OPENCODE_CONFIG_DIR: process.env.OPENCODE_CONFIG_DIR,
            XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
            HOME: process.env.HOME,
        };
        process.env.OPENCODE_CONFIG_DIR = userConfigDir;
        process.env.HOME = homeDir;
        delete process.env.XDG_CONFIG_HOME;
    });

    afterEach(() => {
        for (const [key, value] of Object.entries(originalEnv)) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
        try {
            rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* */
        }
    });

    it("preserves JSONC comments and tuple plugin entries while removing canonical DCP", () => {
        const configPath = join(projectDir, "opencode.jsonc");
        writeFileSync(
            configPath,
            `{
  // The JSONC rewrite preserves this file-level comment.
  "plugin": [
    ["@plannotator/opencode@latest", { "workflow": "plan-agent" }],
    ["@tarquinen/opencode-dcp@latest", { "enabled": true }],
    "@eidnara/opencode@latest"
  ],
  "compaction": {
    // The JSONC rewrite preserves this compaction comment.
    "auto": true,
    "prune": true
  }
}
`,
        );

        const actions = fixConflicts(projectDir, {
            compactionAuto: true,
            compactionPrune: true,
            dcpPlugin: true,
            ...noOmoConflicts,
        });

        const updatedText = readFileSync(configPath, "utf-8");
        const updated = parseJsonc(updatedText) as Record<string, unknown>;
        expect(actions).toEqual([
            "Disabled auto-compaction",
            "Disabled prune",
            "Removed opencode-dcp plugin",
        ]);
        expect(updatedText).toContain("The JSONC rewrite preserves this file-level comment");
        expect(updatedText).toContain("The JSONC rewrite preserves this compaction comment");
        expect(updated.compaction).toEqual({ auto: false, prune: false });
        expect(updated.plugin).toEqual([
            ["@plannotator/opencode@latest", { workflow: "plan-agent" }],
            "@eidnara/opencode@latest",
        ]);
    });

    it("skips non-existent target files instead of creating user config", () => {
        const actions = fixConflicts(projectDir, {
            compactionAuto: true,
            compactionPrune: true,
            dcpPlugin: true,
            ...noOmoConflicts,
        });

        expect(actions).toEqual([]);
        expect(existsSync(join(userConfigDir, "opencode.json"))).toBe(false);
        expect(existsSync(join(userConfigDir, "opencode.jsonc"))).toBe(false);
    });

    it("keeps DCP forks and substring-only names because matching is canonical", () => {
        const configPath = join(projectDir, "opencode.json");
        writeFileSync(
            configPath,
            JSON.stringify({
                plugin: [
                    "@some-fork/opencode-dcp-fork",
                    "file:///tmp/opencode-dcp-dev",
                    ["@other/opencode-dcp-slim@latest", { enabled: true }],
                ],
            }),
        );

        const actions = fixConflicts(projectDir, {
            compactionAuto: false,
            compactionPrune: false,
            dcpPlugin: true,
            ...noOmoConflicts,
        });

        const updated = parseJsonc(readFileSync(configPath, "utf-8")) as Record<string, unknown>;
        expect(actions).toEqual([]);
        expect(updated.plugin).toEqual([
            "@some-fork/opencode-dcp-fork",
            "file:///tmp/opencode-dcp-dev",
            ["@other/opencode-dcp-slim@latest", { enabled: true }],
        ]);
    });

    // oh-my-openagent 4.19.0+ uses the unified OMO config format.

    // The fixer removes DCP from each user-level file that lists it; otherwise
    // detectConflicts keeps the plugin disabled while doctor reports nothing fixed.
    describe("user-level opencode.json and opencode.jsonc both present", () => {
        it("removes DCP from opencode.json when opencode.jsonc also exists", () => {
            const jsoncPath = join(userConfigDir, "opencode.jsonc");
            const jsonPath = join(userConfigDir, "opencode.json");
            writeFileSync(jsoncPath, `{\n  // user theme\n  "theme": "dark"\n}\n`);
            writeFileSync(
                jsonPath,
                JSON.stringify({ plugin: ["@tarquinen/opencode-dcp@latest", "@keep/one"] }),
            );

            const actions = fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: true,
                ...noOmoConflicts,
            });

            expect(actions).toEqual(["Removed opencode-dcp plugin"]);
            expect(JSON.parse(readFileSync(jsonPath, "utf-8")).plugin).toEqual(["@keep/one"]);
            expect(readFileSync(jsoncPath, "utf-8")).toBe(
                `{\n  // user theme\n  "theme": "dark"\n}\n`,
            );
            expect(detectConflicts(projectDir).conflicts.dcpPlugin).toBe(false);
        });

        it("removes DCP from both user files when each lists it", () => {
            const jsoncPath = join(userConfigDir, "opencode.jsonc");
            const jsonPath = join(userConfigDir, "opencode.json");
            writeFileSync(jsoncPath, JSON.stringify({ plugin: ["@tarquinen/opencode-dcp"] }));
            writeFileSync(jsonPath, JSON.stringify({ plugin: ["@tarquinen/opencode-dcp"] }));

            fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: true,
                ...noOmoConflicts,
            });

            expect(JSON.parse(readFileSync(jsoncPath, "utf-8")).plugin).toEqual([]);
            expect(JSON.parse(readFileSync(jsonPath, "utf-8")).plugin).toEqual([]);
        });
    });

    // The host resolves `compaction` by merging layers with later layers winning, so the
    // repair lands in the layer whose value the host uses. Writing into every existing file
    // would disable native compaction machine-wide for a project-scoped conflict.
    describe("compaction repair targets the layer that produced the conflict", () => {
        const compactionConflict = {
            compactionAuto: true,
            compactionPrune: false,
            dcpPlugin: false,
            ...noOmoConflicts,
        };

        it("leaves the user config untouched when the project layer set auto=true", () => {
            const userPath = join(userConfigDir, "opencode.json");
            const userOriginal = `{\n  "theme": "dark"\n}\n`;
            writeFileSync(userPath, userOriginal);
            const projectPath = join(projectDir, "opencode.json");
            writeFileSync(projectPath, JSON.stringify({ compaction: { auto: true } }));

            const actions = fixConflicts(projectDir, compactionConflict);

            expect(actions).toEqual(["Disabled auto-compaction"]);
            expect(readFileSync(userPath, "utf-8")).toBe(userOriginal);
            expect(JSON.parse(readFileSync(projectPath, "utf-8")).compaction).toEqual({
                auto: false,
            });
        });

        it("edits the user config when it is the layer that set auto=true", () => {
            const userPath = join(userConfigDir, "opencode.json");
            writeFileSync(userPath, JSON.stringify({ compaction: { auto: true } }));
            const projectPath = join(projectDir, "opencode.json");
            const projectOriginal = `{\n  "plugin": ["@eidnara/opencode"]\n}\n`;
            writeFileSync(projectPath, projectOriginal);

            const actions = fixConflicts(projectDir, compactionConflict);

            expect(actions).toEqual(["Disabled auto-compaction"]);
            expect(JSON.parse(readFileSync(userPath, "utf-8")).compaction).toEqual({
                auto: false,
            });
            expect(readFileSync(projectPath, "utf-8")).toBe(projectOriginal);
        });

        it("writes the default-derived auto conflict into the highest-precedence existing layer only", () => {
            const userPath = join(userConfigDir, "opencode.json");
            const userOriginal = `{\n  "theme": "dark"\n}\n`;
            writeFileSync(userPath, userOriginal);
            const projectPath = join(projectDir, "opencode.json");
            writeFileSync(projectPath, JSON.stringify({ plugin: ["@eidnara/opencode"] }));

            const actions = fixConflicts(projectDir, compactionConflict);

            expect(actions).toEqual(["Disabled auto-compaction"]);
            expect(readFileSync(userPath, "utf-8")).toBe(userOriginal);
            expect(JSON.parse(readFileSync(projectPath, "utf-8")).compaction).toEqual({
                auto: false,
            });
        });

        it("does not touch prune when only auto conflicts", () => {
            const projectPath = join(projectDir, "opencode.json");
            writeFileSync(
                projectPath,
                JSON.stringify({ compaction: { auto: true, prune: false } }),
            );

            fixConflicts(projectDir, compactionConflict);

            expect(JSON.parse(readFileSync(projectPath, "utf-8")).compaction).toEqual({
                auto: false,
                prune: false,
            });
        });

        it("reports only the prune action when auto was already off", () => {
            const projectPath = join(projectDir, "opencode.json");
            writeFileSync(
                projectPath,
                JSON.stringify({ compaction: { auto: false, prune: true } }),
            );

            const actions = fixConflicts(projectDir, {
                ...compactionConflict,
                compactionAuto: false,
                compactionPrune: true,
            });

            expect(actions).toEqual(["Disabled prune"]);
            expect(JSON.parse(readFileSync(projectPath, "utf-8")).compaction).toEqual({
                auto: false,
                prune: false,
            });
        });

        it("replaces a non-object compaction value instead of throwing", () => {
            const projectPath = join(projectDir, "opencode.json");
            writeFileSync(projectPath, JSON.stringify({ compaction: "legacy", theme: "dark" }));

            const actions = fixConflicts(projectDir, {
                ...compactionConflict,
                compactionPrune: true,
            });

            expect(actions).toEqual(["Disabled auto-compaction", "Disabled prune"]);
            expect(JSON.parse(readFileSync(projectPath, "utf-8"))).toEqual({
                compaction: { auto: false, prune: false },
                theme: "dark",
            });
        });

        it("repairs prune in its own winning layer, separately from auto", () => {
            const userPath = join(userConfigDir, "opencode.json");
            writeFileSync(userPath, JSON.stringify({ compaction: { prune: true } }));
            const projectPath = join(projectDir, "opencode.json");
            writeFileSync(projectPath, JSON.stringify({ compaction: { auto: true } }));

            const actions = fixConflicts(projectDir, {
                ...compactionConflict,
                compactionPrune: true,
            });

            expect(actions).toEqual(["Disabled auto-compaction", "Disabled prune"]);
            expect(JSON.parse(readFileSync(userPath, "utf-8")).compaction).toEqual({
                prune: false,
            });
            expect(JSON.parse(readFileSync(projectPath, "utf-8")).compaction).toEqual({
                auto: false,
            });
            expect(detectConflicts(projectDir).hasConflict).toBe(false);
        });
    });

    describe("unified OMO config paths", () => {
        const omoConflicts = {
            compactionAuto: false,
            compactionPrune: false,
            dcpPlugin: false,
            omoPreemptiveCompaction: true,
            omoContextWindowMonitor: true,
            omoAnthropicRecovery: true,
        };

        it("disables hooks inside [opencode] block in ~/.omo/omo.jsonc, comments survive, detector confirms", () => {
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.jsonc");
            writeFileSync(
                configPath,
                `{
  // top-level comment
  "some-omo-setting": true,
  "[opencode]": {
    // opencode block comment
    "other_setting": "value"
  }
}
`,
            );

            const actions = fixConflicts(projectDir, omoConflicts);

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);

            const updatedText = readFileSync(configPath, "utf-8");
            expect(updatedText).toContain("top-level comment");
            expect(updatedText).toContain("opencode block comment");
            expect(updatedText).toContain("some-omo-setting");

            const updated = parseJsonc(updatedText) as Record<string, unknown>;
            const opencodeBlock = updated["[opencode]"] as Record<string, unknown>;
            expect(opencodeBlock.disabled_hooks).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);
            expect(opencodeBlock.other_setting).toBe("value");

            // detectConflicts requires a project-level opencode.json containing the OMO plugin.
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ plugin: ["oh-my-opencode"] }),
            );
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(false);
            expect(result.conflicts.omoContextWindowMonitor).toBe(false);
            expect(result.conflicts.omoAnthropicRecovery).toBe(false);
        });

        it("creates [opencode] block when missing in unified omo.jsonc", () => {
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.jsonc");
            writeFileSync(
                configPath,
                `{
  // top-level setting
  "some-omo-setting": true
}
`,
            );

            const actions = fixConflicts(projectDir, omoConflicts);

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);

            const updatedText = readFileSync(configPath, "utf-8");
            expect(updatedText).toContain("top-level setting");

            const updated = parseJsonc(updatedText) as Record<string, unknown>;
            const opencodeBlock = updated["[opencode]"] as Record<string, unknown>;
            expect(opencodeBlock.disabled_hooks).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);
        });

        it("writes to project-level .omo/omo.jsonc", () => {
            const omoDir = join(projectDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.jsonc");
            writeFileSync(
                configPath,
                JSON.stringify({
                    "[opencode]": {},
                }),
            );

            const actions = fixConflicts(projectDir, omoConflicts);

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);

            const updated = parseJsonc(readFileSync(configPath, "utf-8")) as Record<
                string,
                unknown
            >;
            const opencodeBlock = updated["[opencode]"] as Record<string, unknown>;
            expect(opencodeBlock.disabled_hooks).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);
        });

        it("reads omo.json (fallback) when omo.jsonc does not exist", () => {
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.json");
            writeFileSync(
                configPath,
                JSON.stringify({
                    "[opencode]": {},
                }),
            );

            const actions = fixConflicts(projectDir, omoConflicts);

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);

            const updated = JSON.parse(readFileSync(configPath, "utf-8"));
            expect(updated["[opencode]"].disabled_hooks).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);
        });

        it("writes only omo.jsonc when omo.json sits beside it, like OMO reads it", () => {
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const jsoncPath = join(omoDir, "omo.jsonc");
            const jsonPath = join(omoDir, "omo.json");
            const jsonOriginal = JSON.stringify({ "[opencode]": {} });
            writeFileSync(jsoncPath, JSON.stringify({ "[opencode]": {} }));
            writeFileSync(jsonPath, jsonOriginal);

            const actions = fixConflicts(projectDir, omoConflicts);

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);
            expect(
                JSON.parse(readFileSync(jsoncPath, "utf-8"))["[opencode]"].disabled_hooks,
            ).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);
            expect(readFileSync(jsonPath, "utf-8")).toBe(jsonOriginal);
        });

        it.each([
            ["null", "null"],
            ["boolean", "true"],
            ["string", '"x"'],
            ["array", "[]"],
        ])("replaces a %s `[opencode]` value instead of throwing, and the detector confirms", (_kind, literal) => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ plugin: ["oh-my-opencode"] }),
            );
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.jsonc");
            writeFileSync(
                configPath,
                `{\n  // keep\n  "[opencode]": ${literal},\n  "other": 1\n}\n`,
            );

            let actions: string[] = [];
            expect(() => {
                actions = fixConflicts(projectDir, omoConflicts);
            }).not.toThrow();

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);
            const text = readFileSync(configPath, "utf-8");
            expect(text).toContain("// keep");
            const updated = parseJsonc(text) as Record<string, unknown>;
            expect(updated.other).toBe(1);
            expect(updated["[opencode]"]).toEqual({
                disabled_hooks: [
                    "context-window-monitor",
                    "preemptive-compaction",
                    "anthropic-context-window-limit-recovery",
                ],
            });
            const after = detectConflicts(projectDir).conflicts;
            expect(after.omoPreemptiveCompaction).toBe(false);
            expect(after.omoContextWindowMonitor).toBe(false);
            expect(after.omoAnthropicRecovery).toBe(false);
        });

        it("updates both legacy and unified config when both exist", () => {
            // The fixer supports project-level .opencode/oh-my-opencode.json as a legacy configuration format.
            const legacyDir = join(projectDir, ".opencode");
            mkdirSync(legacyDir, { recursive: true });
            const legacyPath = join(legacyDir, "oh-my-opencode.json");
            writeFileSync(legacyPath, JSON.stringify({ disabled_hooks: [] }));

            // Unified: ~/.omo/omo.jsonc
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const unifiedPath = join(omoDir, "omo.jsonc");
            writeFileSync(
                unifiedPath,
                JSON.stringify({
                    "[opencode]": {},
                }),
            );

            const actions = fixConflicts(projectDir, omoConflicts);

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);

            // The legacy configuration stores disabled_hooks at the top level.
            const legacy = JSON.parse(readFileSync(legacyPath, "utf-8"));
            expect(legacy.disabled_hooks).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);

            // The unified configuration stores disabled_hooks in the [opencode] block.
            const unified = JSON.parse(readFileSync(unifiedPath, "utf-8"));
            expect(unified["[opencode]"].disabled_hooks).toEqual([
                "context-window-monitor",
                "preemptive-compaction",
                "anthropic-context-window-limit-recovery",
            ]);
        });

        it("skips non-existent unified paths (no create)", () => {
            // Without a .omo directory, the fixer finds no targets.
            const actions = fixConflicts(projectDir, omoConflicts);
            expect(actions).toEqual([]);
        });
    });

    describe("compaction-off mode parity (issue #266)", () => {
        it("does NOT flip compaction.auto to false when compaction-off", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            const original = `{
  // keep comment
  "compaction": {
    "auto": true,
    "prune": true
  }
}
`;
            writeFileSync(configPath, original);

            const actions = fixConflicts(
                projectDir,
                {
                    compactionAuto: true,
                    compactionPrune: true,
                    dcpPlugin: false,
                    ...noOmoConflicts,
                },
                { compactionEnabled: false },
            );

            expect(actions).toEqual([]);
            expect(readFileSync(configPath, "utf-8")).toBe(original);
        });

        it("still removes DCP plugin in compaction-off mode (DCP policy is mode-independent)", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            writeFileSync(
                configPath,
                JSON.stringify({
                    plugin: ["@tarquinen/opencode-dcp@latest"],
                    compaction: { auto: true, prune: true },
                }),
            );

            const actions = fixConflicts(
                projectDir,
                { compactionAuto: true, compactionPrune: true, dcpPlugin: true, ...noOmoConflicts },
                { compactionEnabled: false },
            );

            expect(actions).toEqual(["Removed opencode-dcp plugin"]);
            const updated = parseJsonc(readFileSync(configPath, "utf-8")) as Record<
                string,
                unknown
            >;
            expect(updated.compaction).toEqual({ auto: true, prune: true });
            expect(updated.plugin).toEqual([]);
        });

        it("still disables OMO hooks in compaction-off mode (OMO policy is mode-independent)", () => {
            const configPath = join(projectDir, "opencode.json");
            writeFileSync(configPath, JSON.stringify({ plugin: ["oh-my-opencode"] }));
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const omoPath = join(omoDir, "omo.jsonc");
            writeFileSync(omoPath, JSON.stringify({ "[opencode]": {} }));

            const actions = fixConflicts(
                projectDir,
                {
                    compactionAuto: false,
                    compactionPrune: false,
                    dcpPlugin: false,
                    omoPreemptiveCompaction: true,
                    omoContextWindowMonitor: true,
                    omoAnthropicRecovery: true,
                },
                { compactionEnabled: false },
            );

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);
        });

        it("mutation direction: same conflict IS repaired when mode forced on", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            writeFileSync(configPath, JSON.stringify({ compaction: { auto: true, prune: true } }));

            const offActions = fixConflicts(
                projectDir,
                {
                    compactionAuto: true,
                    compactionPrune: true,
                    dcpPlugin: false,
                    ...noOmoConflicts,
                },
                { compactionEnabled: false },
            );
            const onActions = fixConflicts(
                projectDir,
                {
                    compactionAuto: true,
                    compactionPrune: true,
                    dcpPlugin: false,
                    ...noOmoConflicts,
                },
                { compactionEnabled: true },
            );

            expect(offActions).toEqual([]);
            expect(onActions).toEqual(["Disabled auto-compaction", "Disabled prune"]);
            const updated = parseJsonc(readFileSync(configPath, "utf-8")) as Record<
                string,
                unknown
            >;
            expect(updated.compaction).toEqual({ auto: false, prune: false });
        });
    });

    describe("JSONC byte preservation", () => {
        it("writes through a same-directory temp file and keeps the destination's mode", () => {
            const configPath = join(projectDir, "opencode.json");
            writeFileSync(configPath, JSON.stringify({ compaction: { auto: true } }));
            chmodSync(configPath, 0o600);

            fixConflicts(projectDir, {
                compactionAuto: true,
                compactionPrune: false,
                dcpPlugin: false,
                ...noOmoConflicts,
            });

            expect(JSON.parse(readFileSync(configPath, "utf-8")).compaction).toEqual({
                auto: false,
            });
            expect(statSync(configPath).mode & 0o777).toBe(0o600);
            expect(existsSync(`${configPath}.tmp`)).toBe(false);
        });

        it("removes DCP and disables compaction without changing comments or formatting elsewhere", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            const original =
                "// leading file comment\r\n" +
                "{\r\n" +
                '\t"plugin": [\r\n' +
                "\t\t// first plugin comment\r\n" +
                '\t\t"@keep/first",\r\n' +
                "\t\t// removed DCP comment\r\n" +
                '\t\t"@tarquinen/opencode-dcp@latest",\r\n' +
                "\t\t// second plugin comment\r\n" +
                '\t\t"@keep/second",\r\n' +
                "\t], // trailing array comment\r\n" +
                '\t"compaction": {\r\n' +
                "\t\t// nested compaction comment\r\n" +
                '\t\t"auto": true,\r\n' +
                '\t\t"prune": true,\r\n' +
                "\t},\r\n" +
                '\t"theme": "dark",\r\n' +
                "}\r\n";
            const expected =
                "// leading file comment\r\n" +
                "{\r\n" +
                '\t"plugin": [\r\n' +
                "\t\t// first plugin comment\r\n" +
                '\t\t"@keep/first",\r\n' +
                "\t\t// second plugin comment\r\n" +
                '\t\t"@keep/second",\r\n' +
                "\t], // trailing array comment\r\n" +
                '\t"compaction": {\r\n' +
                "\t\t// nested compaction comment\r\n" +
                '\t\t"auto": false,\r\n' +
                '\t\t"prune": false,\r\n' +
                "\t},\r\n" +
                '\t"theme": "dark",\r\n' +
                "}\r\n";
            writeFileSync(configPath, original);

            const actions = fixConflicts(projectDir, {
                compactionAuto: true,
                compactionPrune: true,
                dcpPlugin: true,
                ...noOmoConflicts,
            });

            expect(actions).toEqual([
                "Disabled auto-compaction",
                "Disabled prune",
                "Removed opencode-dcp plugin",
            ]);
            expect(readFileSync(configPath, "utf-8")).toBe(expected);
            expect(readFileSync(configPath, "utf-8")).not.toContain("removed DCP comment");
        });

        it("keeps each survivor's own comment when removing a middle plugin entry", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            const original = `{
    "plugin": [
        // first plugin
        "@keep/first",
        // removed plugin
        "@tarquinen/opencode-dcp@latest",
        // second plugin
        "@keep/second",
    ],
    "theme": "dark",
}
`;
            const expected = `{
    "plugin": [
        // first plugin
        "@keep/first",
        // second plugin
        "@keep/second",
    ],
    "theme": "dark",
}
`;
            writeFileSync(configPath, original);

            fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: true,
                ...noOmoConflicts,
            });

            expect(readFileSync(configPath, "utf-8")).toBe(expected);
        });

        it("preserves CRLF and the array's trailing comment when DCP is the final entry", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            const original =
                "{\r\n" +
                '\t"plugin": [\r\n' +
                "\t\t// surviving plugin\r\n" +
                '\t\t"@keep/first",\r\n' +
                "\t\t// removed final plugin\r\n" +
                '\t\t"@tarquinen/opencode-dcp@latest",\r\n' +
                "\t], // array trailing comment\r\n" +
                "}\r\n";
            const expected =
                "{\r\n" +
                '\t"plugin": [\r\n' +
                "\t\t// surviving plugin\r\n" +
                '\t\t"@keep/first"\r\n' +
                "\t], // array trailing comment\r\n" +
                "}\r\n";
            writeFileSync(configPath, original);

            fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: true,
                ...noOmoConflicts,
            });

            expect(readFileSync(configPath, "utf-8")).toBe(expected);
        });

        it("leaves an already conflict-free config byte-identical", () => {
            const configPath = join(projectDir, "opencode.jsonc");
            const original = '// preserve every byte\r\n{\r\n\t"plugin": ["@keep/one",],\r\n}\r\n';
            writeFileSync(configPath, original);

            const actions = fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: true,
                ...noOmoConflicts,
            });

            expect(actions).toEqual([]);
            expect(readFileSync(configPath, "utf-8")).toBe(original);
        });

        it("appends OMO hooks without reformatting the existing JSONC", () => {
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.jsonc");
            const original = `{
    // The fixer preserves this leading comment.
    "[opencode]": {
        "disabled_hooks": [
            // The fixer preserves this custom-hook comment.
            "custom-hook",
        ], // preserve this array comment
    },
}
`;
            const expected = `{
    // The fixer preserves this leading comment.
    "[opencode]": {
        "disabled_hooks": [
            // The fixer preserves this custom-hook comment.
            "custom-hook",
            "context-window-monitor",
        ], // preserve this array comment
    },
}
`;
            writeFileSync(configPath, original);

            const actions = fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: false,
                omoPreemptiveCompaction: false,
                omoContextWindowMonitor: true,
                omoAnthropicRecovery: false,
            });

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);
            expect(readFileSync(configPath, "utf-8")).toBe(expected);
        });

        it("appends to a multiline array whose closing bracket shares the last entry's line", () => {
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            const configPath = join(omoDir, "omo.jsonc");
            const original = `{
  "[opencode]": {
    "disabled_hooks": [
      "a",
      "b"]
  }
}
`;
            const expected = `{
  "[opencode]": {
    "disabled_hooks": [
      "a",
      "b","context-window-monitor"]
  }
}
`;
            writeFileSync(configPath, original);

            const actions = fixConflicts(projectDir, {
                compactionAuto: false,
                compactionPrune: false,
                dcpPlugin: false,
                omoPreemptiveCompaction: false,
                omoContextWindowMonitor: true,
                omoAnthropicRecovery: false,
            });

            expect(actions).toEqual(["Disabled conflicting oh-my-opencode hooks"]);
            const text = readFileSync(configPath, "utf-8");
            expect(text).toBe(expected);
            expect(
                (parseJsonc(text) as Record<string, Record<string, unknown>>)["[opencode]"]
                    .disabled_hooks,
            ).toEqual(["a", "b", "context-window-monitor"]);
        });

        it("skips a file with duplicate object keys instead of editing the shadowed value", () => {
            const configPath = join(projectDir, "opencode.json");
            // JSON parsing keeps the second `compaction`, so the effective value is `auto: true`.
            const original = `{"compaction":{"auto":false},"compaction":{"auto":true}}`;
            writeFileSync(configPath, original);
            expect(
                detectConflicts(projectDir, { compactionEnabled: true }).conflicts.compactionAuto,
            ).toBe(true);

            const actions = fixConflicts(projectDir, {
                compactionAuto: true,
                compactionPrune: false,
                dcpPlugin: false,
                ...noOmoConflicts,
            });

            expect(actions).toEqual([]);
            expect(readFileSync(configPath, "utf-8")).toBe(original);
        });
    });
});
