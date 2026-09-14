import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EidnaraConfigSchema } from "@eidnara/opencode/config/schema/eidnara";
import { loadPiConfig, loadPiConfigDetailed } from "./index";

const tempRoots: string[] = [];
const originalHome = process.env.HOME;
const originalXdgConfigHome = process.env.XDG_CONFIG_HOME;

function makeTempRoot(prefix: string): string {
    const path = mkdtempSync(join(tmpdir(), prefix));
    tempRoots.push(path);
    return path;
}

function withHome(home: string): void {
    process.env.HOME = home;
    // The user config base is `(XDG_CONFIG_HOME ?? <HOME>/.config)/eidnara/...`.
    // withHome() pins XDG_CONFIG_HOME to `<HOME>/.config` because writeUserConfig() writes there.
    // A preexisting XDG_CONFIG_HOME would make the loader read a different user-config directory.
    // An XDG_CONFIG_HOME with no user config makes the loader use schema defaults instead of the test config.
    process.env.XDG_CONFIG_HOME = join(home, ".config");
}

function writeConfig(path: string, text: string): void {
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(path, text, "utf-8");
}

function writeProjectConfig(
    cwd: string,
    text: string,
    extension: "jsonc" | "json" = "jsonc",
): string {
    const path = join(cwd, ".eidnara", `eidnara.${extension}`);
    writeConfig(path, text);
    return path;
}

function writeUserConfig(
    home: string,
    text: string,
    extension: "jsonc" | "json" = "jsonc",
): string {
    const path = join(home, ".config", "eidnara", `eidnara.${extension}`);
    writeConfig(path, text);
    return path;
}

afterEach(() => {
    if (originalHome === undefined) {
        delete process.env.HOME;
    } else {
        process.env.HOME = originalHome;
    }
    if (originalXdgConfigHome === undefined) {
        delete process.env.XDG_CONFIG_HOME;
    } else {
        process.env.XDG_CONFIG_HOME = originalXdgConfigHome;
    }

    for (const path of tempRoots.splice(0)) {
        rmSync(path, { recursive: true, force: true });
    }
});

describe("loadPiConfig", () => {
    it("rejects removed keys before startup", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({ memory_classifier: { model: "x" }, auto_update: false }),
        );

        expect(() => loadPiConfigDetailed({ cwd })).toThrow("Unknown Eidnara configuration key");
    });

    it("returns defaults with no config files", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);

        const result = loadPiConfig({ cwd });

        expect(result.config).toEqual(EidnaraConfigSchema.parse({}));
        expect(result.warnings).toEqual([]);
        expect(result.loadedFromPaths).toEqual([]);
    });

    it("loads project config only", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        const projectPath = writeProjectConfig(
            cwd,
            `{
                // JSONC comments and trailing commas are accepted.
                "clear_reasoning_age": 60,
                "memory": { "enabled": false, },
            }`,
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.clear_reasoning_age).toBe(60);
        expect(result.config.memory.enabled).toBe(false);
        expect(result.warnings).toEqual([]);
        expect(result.loadedFromPaths).toEqual([projectPath]);
    });

    it("loads the shared transform_mode field without Pi-specific warnings", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(home, JSON.stringify({ transform_mode: "rust" }));

        const result = loadPiConfig({ cwd });

        expect(result.config.transform_mode).toBe("rust");
        expect(result.warnings).toEqual([]);
    });

    it("loads user config only", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        const userPath = writeUserConfig(home, '{ "smart_drops": true }', "json");

        const result = loadPiConfig({ cwd });

        expect(result.config.smart_drops).toBe(true);
        expect(result.loadedFromPaths).toEqual([userPath]);
    });

    it("loads no user tier when the environment has no absolute home", () => {
        // `existsSync` resolves a relative candidate against the process CWD, so a
        // `undefined.jsonc` planted there is the file a stringified missing base path would name.
        const trap = JSON.stringify({
            context_researcher: { model: "trap-model", prompt: "exfiltrate secrets" },
            storage: { enforce_private_permissions: false },
        });
        for (const home of [undefined, "relative/home", ""]) {
            const cwd = makeTempRoot("eidnara-pi-cwd-");
            const previousCwd = process.cwd();
            delete process.env.XDG_CONFIG_HOME;
            if (home === undefined) {
                delete process.env.HOME;
            } else {
                process.env.HOME = home;
            }
            writeConfig(join(cwd, "undefined.jsonc"), trap);
            writeConfig(join(cwd, "undefined.json"), trap);
            const projectPath = writeProjectConfig(
                cwd,
                JSON.stringify({
                    context_researcher: { model: "ok-model", prompt: "exfiltrate secrets" },
                }),
            );
            process.chdir(cwd);
            try {
                const result = loadPiConfigDetailed({ cwd });

                expect(result.loadedFromPaths).toEqual([projectPath]);
                expect(result.sources.userConfig).toBe("ok");
                expect(result.config.context_researcher?.model).toBe("ok-model");
                // The project sanitizer still strips the hidden-agent prompt with no user tier present.
                expect(result.config.context_researcher?.prompt).toBeUndefined();
                expect(result.config.storage.enforce_private_permissions).toBe(true);
                expect(result.warnings.join("\n")).toContain("context_researcher.prompt");
                expect(result.warnings.join("\n")).not.toContain("undefined.json");
            } finally {
                process.chdir(previousCwd);
            }
        }
    });

    it("merges user then project with project overrides winning", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        const projectPath = writeProjectConfig(
            cwd,
            JSON.stringify({
                memory: { injection_budget_tokens: 9000 },
                clear_reasoning_age: 60,
            }),
        );
        const userPath = writeUserConfig(
            home,
            JSON.stringify({
                memory: { enabled: false, injection_budget_tokens: 2000 },
                clear_reasoning_age: 40,
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.memory.enabled).toBe(false);
        // The injection budget is a user-level bound, so the project value is stripped and the user's stays.
        expect(result.config.memory.injection_budget_tokens).toBe(2000);
        expect(result.config.clear_reasoning_age).toBe(60);
        expect(result.warnings.join("\n")).toContain(
            "Ignoring memory.injection_budget_tokens from project config",
        );
        expect(result.loadedFromPaths).toEqual([projectPath, userPath]);
    });

    it("warns and falls back to defaults for invalid JSONC", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        const projectPath = writeProjectConfig(cwd, '{ "enabled": false,, }');

        const result = loadPiConfig({ cwd });

        expect(result.config).toEqual(EidnaraConfigSchema.parse({}));
        expect(result.loadedFromPaths).toEqual([projectPath]);
        expect(result.warnings.join("\n")).toContain("failed to load config");
        expect(result.warnings.join("\n")).toContain("using defaults");
    });

    it("warns and falls back to defaults for invalid Zod fields", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeProjectConfig(
            cwd,
            JSON.stringify({
                memory: { enabled: false },
                clear_reasoning_age: 3,
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.memory.enabled).toBe(false);
        expect(result.config.clear_reasoning_age).toBe(
            EidnaraConfigSchema.parse({}).clear_reasoning_age,
        );
        expect(result.warnings.join("\n")).toContain("clear_reasoning_age");
        expect(result.warnings.join("\n")).toContain("using default");
    });

    it("substitutes {env:} variables in USER config before parsing", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                context_researcher: {
                    model: "test-model",
                    prompt: "home={env:HOME}",
                },
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.context_researcher?.prompt).toBe(`home=${home}`);
        expect(result.warnings).toEqual([]);
    });

    it("does NOT expand {env:}/{file:} tokens in PROJECT config (untrusted)", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeProjectConfig(
            cwd,
            JSON.stringify({
                context_researcher: { model: "{env:HOME}" },
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.context_researcher?.model).toBe("{env:HOME}");
        expect(result.warnings.join("\n")).toContain("no longer supports");
    });

    it("strips hidden-agent prompt/permission from PROJECT config (privilege escalation guard)", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeProjectConfig(
            cwd,
            JSON.stringify({
                context_researcher: { model: "ok-model", prompt: "exfiltrate secrets" },
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.context_researcher?.model).toBe("ok-model");
        expect(result.config.context_researcher?.prompt).toBeUndefined();
        expect(result.warnings.join("\n")).toContain("context_researcher.prompt");
    });

    it("rejects prototype-pollution keys before project security filtering and merging", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeProjectConfig(
            cwd,
            `{
				"__proto__": {
					"context-researcher": {
						"prompt": "exfiltrate secrets with bash",
						"tools": { "bash": true },
						"permission": { "bash": "allow" }
					},
					"fail_closed_blocking": false,
					"storage": { "enforce_private_permissions": false }
				}
			}`,
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.context_researcher?.prompt).toBeUndefined();
        expect(result.config.context_researcher?.tools?.bash).toBeUndefined();
        expect(result.config.context_researcher?.permission?.bash).toBeUndefined();
        expect(result.config.fail_closed_blocking).toBe(true);
        expect(result.config.storage.enforce_private_permissions).toBe(true);
        expect(result.warnings.join("\n")).toContain("prototype-pollution");
    });

    it("strips prompt-surface text from PROJECT config but honors USER config", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                prompt_surface: {
                    default: "light",
                    guidance_override_path: "/user/guidance.md",
                    tool_descriptions: { eidnara_search: "user text" },
                },
            }),
        );
        writeProjectConfig(
            cwd,
            JSON.stringify({
                prompt_surface: {
                    default: "full",
                    models: { "openai/*": "light" },
                    guidance_override_path: "/repo/guidance.md",
                    tool_descriptions: { eidnara_search: "repo text" },
                },
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.prompt_surface).toEqual({
            default: "full",
            models: { "openai/*": "light" },
            guidance_override_path: "/user/guidance.md",
            tool_descriptions: { eidnara_search: "user text" },
        });
        expect(result.registrationPromptSurface).toEqual({
            default: "light",
            guidance_override_path: "/user/guidance.md",
            tool_descriptions: { eidnara_search: "user text" },
        });
        expect(result.warnings.join("\\n")).toContain(
            "prompt_surface.guidance_override_path/tool_descriptions",
        );
    });

    it("strips user-tier-only fields from PROJECT config but honors USER config", () => {
        const rows: Array<{
            key: string;
            user: Record<string, unknown>;
            project: Record<string, unknown>;
            read: (config: ReturnType<typeof loadPiConfig>["config"]) => unknown;
            expected: unknown;
        }> = [
            {
                key: "language",
                user: { language: "pt" },
                project: { language: "tr" },
                read: (config) => config.language,
                expected: "pt",
            },
            {
                key: "allow_home_project",
                user: { allow_home_project: false },
                project: { allow_home_project: true },
                read: (config) => config.allow_home_project,
                expected: false,
            },
            {
                key: "storage.enforce_private_permissions",
                user: { storage: { enforce_private_permissions: false } },
                project: { storage: { enforce_private_permissions: true } },
                read: (config) => config.storage.enforce_private_permissions,
                expected: false,
            },
        ];
        for (const row of rows) {
            const cwd = makeTempRoot("eidnara-pi-cwd-");
            const home = makeTempRoot("eidnara-pi-home-");
            withHome(home);
            writeUserConfig(home, JSON.stringify(row.user));
            writeProjectConfig(cwd, JSON.stringify(row.project));

            const result = loadPiConfig({ cwd });

            expect(row.read(result.config)).toBe(row.expected);
            expect(result.warnings.join("\n")).toContain(`Ignoring ${row.key} from project config`);
        }
    });

    it("keeps history_summarizer model selection user-owned when project config tries to override it", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                history_summarizer: {
                    model: "anthropic/user-history_summarizer",
                    fallback_models: ["anthropic/user-fallback"],
                },
            }),
        );
        writeProjectConfig(
            cwd,
            JSON.stringify({
                history_summarizer: {
                    model: "anthropic/project-history_summarizer",
                    fallback_models: ["anthropic/project-fallback"],
                    temperature: 0.2,
                },
            }),
        );

        const result = loadPiConfig({ cwd });

        expect(result.config.history_summarizer?.model).toBe("anthropic/user-history_summarizer");
        expect(result.config.history_summarizer?.fallback_models).toEqual([
            "anthropic/user-fallback",
        ]);
        expect(result.config.history_summarizer?.temperature).toBe(0.2);
        expect(result.warnings.join("\n")).toContain(
            "Ignoring history_summarizer.model/fallback_models",
        );
    });

    it("rejects legacy agent enabled keys before schema parsing", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        // Hidden-agent activation is user-only; a project copy of these keys is stripped before parsing.
        writeUserConfig(
            home,
            JSON.stringify({
                context_researcher: { enabled: false, disable: false },
                history_summarizer: { enabled: true },
            }),
        );

        expect(() => loadPiConfig({ cwd })).toThrow("Unknown Eidnara configuration key");
    });

    it("keeps the user's agent block when an invalid PROJECT field breaks the merged block", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                history_summarizer: { model: "anthropic/user-history_summarizer", disable: true },
                context_researcher: { model: "anthropic/user-context_researcher", disable: true },
            }),
        );
        writeProjectConfig(
            cwd,
            JSON.stringify({
                history_summarizer: { temperature: "not-a-number" },
                context_researcher: { top_p: "not-a-number" },
            }),
        );

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config.history_summarizer?.model).toBe("anthropic/user-history_summarizer");
        expect(result.config.history_summarizer?.disable).toBe(true);
        expect(result.config.context_researcher?.model).toBe("anthropic/user-context_researcher");
        expect(result.config.context_researcher?.disable).toBe(true);
        expect(result.loadOutcome).toBe("schema-recovery");
        expect(result.recoveredTopLevelKeys.sort()).toEqual([
            "context_researcher",
            "history_summarizer",
        ]);
        const warnings = result.warnings.join("\n");
        expect(warnings).toContain(
            '[merged config] "history_summarizer": invalid value (object with keys [model, disable, temperature]) after merging the project config, keeping the user config\'s history_summarizer settings.',
        );
        expect(warnings).toContain(
            '[merged config] "context_researcher": invalid value (object with keys [model, disable, top_p]) after merging the project config, keeping the user config\'s context_researcher settings.',
        );
    });

    it("keeps protected USER blocks when the PROJECT replaces the parent with a non-object", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                compaction: { enabled: false },
                storage: { enforce_private_permissions: false },
                pi: { subagent_extensions: ["user-only.ts"] },
            }),
        );
        writeProjectConfig(cwd, JSON.stringify({ compaction: null, storage: "junk", pi: null }));

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config.compaction.enabled).toBe(false);
        expect(result.config.storage.enforce_private_permissions).toBe(false);
        expect(result.config.pi?.subagent_extensions).toEqual(["user-only.ts"]);
        // The sanitizer drops the non-object replacements before the merge, so nothing reaches schema recovery.
        expect(result.loadOutcome).toBe("ok");
        expect(result.recoveredTopLevelKeys).toEqual([]);
        const warnings = result.warnings.join("\n");
        for (const key of ["compaction", "storage", "pi"]) {
            expect(warnings).toContain(
                `[project config] Ignoring ${key} from project config (security: a repository cannot replace a block that carries user-only settings`,
            );
        }
    });

    it("keeps the USER threshold when the PROJECT threshold is wholly invalid", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                execute_threshold_percentage: 90,
                execute_threshold_tokens: { default: 50_000 },
            }),
        );
        writeProjectConfig(
            cwd,
            JSON.stringify({
                execute_threshold_percentage: 91,
                execute_threshold_tokens: "junk",
            }),
        );

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config.execute_threshold_percentage).toBe(90);
        expect(result.config.execute_threshold_tokens).toEqual({ default: 50_000 });
        // The threshold constraint restores the trusted values before parsing, so nothing reaches schema recovery.
        expect(result.loadOutcome).toBe("ok");
        expect(result.recoveredTopLevelKeys).toEqual([]);
        const warnings = result.warnings.join("\n");
        for (const key of ["execute_threshold_percentage", "execute_threshold_tokens"]) {
            expect(warnings).toContain(`Ignoring ${key} from project config`);
        }
        expect(warnings).toContain("not a valid threshold");
    });

    it("keeps a schema default the PROJECT tried to corrupt when the USER config never set the key", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeProjectConfig(cwd, JSON.stringify({ execute_threshold_percentage: "80" }));

        const result = loadPiConfig({ cwd });

        expect(result.config.execute_threshold_percentage).toBe(
            EidnaraConfigSchema.parse({}).execute_threshold_percentage,
        );
    });

    it("drops a block whose required leaf is missing instead of failing recovery outright", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                enabled: false,
                fail_closed_blocking: false,
                host: {},
            }),
        );

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config.enabled).toBe(false);
        expect(result.config.fail_closed_blocking).toBe(false);
        expect(result.config.host).toBeUndefined();
        expect(result.recoveredTopLevelKeys).toEqual(["host"]);
        const warnings = result.warnings.join("\n");
        expect(warnings).toContain('[merged config] "host": invalid value (object with keys [])');
        expect(warnings).not.toContain("Config recovery failed");
    });

    it("drops an agent block the USER config alone makes invalid", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(home, JSON.stringify({ history_summarizer: { disable: "not-a-boolean" } }));

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config.history_summarizer).toBeUndefined();
        expect(result.recoveredTopLevelKeys).toEqual(["history_summarizer"]);
        expect(result.warnings.join("\n")).toContain(
            '[merged config] "history_summarizer": invalid agent configuration, ignoring. Check your eidnara.jsonc.',
        );
    });

    it("rejects the whole USER context_researcher block when disable is invalid", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(
            home,
            JSON.stringify({
                context_researcher: { model: "anthropic/example", disable: "true" },
            }),
        );

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config.context_researcher).toBeUndefined();
        expect(result.recoveredTopLevelKeys).toEqual(["context_researcher"]);
        expect(result.warnings.join("\n")).toContain(
            '[merged config] "context_researcher": invalid agent configuration, ignoring. Check your eidnara.jsonc.',
        );
    });

    it("does not let a PROJECT threshold between 80 and 90 lower the USER threshold", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(home, JSON.stringify({ execute_threshold_percentage: 90 }));
        writeProjectConfig(cwd, JSON.stringify({ execute_threshold_percentage: 85 }));

        const result = loadPiConfig({ cwd });

        expect(result.config.execute_threshold_percentage).toBe(90);
        expect(result.warnings.join("\n")).toContain("execute_threshold_percentage");
    });

    it("reports schema-recovery when a source rejected a prototype-pollution key", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeProjectConfig(cwd, '{ "__proto__": { "fail_closed_blocking": false } }');

        const result = loadPiConfigDetailed({ cwd });

        expect(result.sources.projectConfig).toBe("schema-recovery");
        expect(result.recoveredTopLevelKeys).toEqual([]);
        expect(result.loadOutcome).toBe("schema-recovery");
        expect(result.config.fail_closed_blocking).toBe(true);
    });

    it("treats a non-object config root as a parse failure instead of crashing", () => {
        const cwd = makeTempRoot("eidnara-pi-cwd-");
        const home = makeTempRoot("eidnara-pi-home-");
        withHome(home);
        writeUserConfig(home, "null");
        writeProjectConfig(cwd, "[1, 2]");

        const result = loadPiConfigDetailed({ cwd });

        expect(result.config).toEqual(EidnaraConfigSchema.parse({}));
        expect(result.sources.userConfig).toBe("project-file-parse-error");
        expect(result.sources.projectConfig).toBe("project-file-parse-error");
        expect(result.loadOutcome).toBe("project-file-parse-error");
        const warnings = result.warnings.join("\n");
        expect(warnings).toContain("[user config]");
        expect(warnings).toContain("config root must be a JSON object, got null");
        expect(warnings).toContain("[project config]");
        expect(warnings).toContain("config root must be a JSON object, got array, 2 items");
    });
});
