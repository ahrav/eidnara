import { Database } from "bun:sqlite";
import { afterEach, describe, expect, it, setDefaultTimeout } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    collectDiagnostics,
    collectHistorianDumps,
    renderDiagnosticsMarkdown,
} from "./diagnostics-opencode";

setDefaultTimeout(15_000);

const tempRoots: string[] = [];
const ENV_KEYS = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_DB_PATH",
] as const;
const originalEnv = Object.fromEntries(ENV_KEYS.map((key) => [key, process.env[key]]));

afterEach(() => {
    for (const key of ENV_KEYS) {
        const value = originalEnv[key];
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function isolatedRoot(): { root: string; configHome: string; cwd: string } {
    const root = mkdtempSync(join(tmpdir(), "eidnara-opencode-diagnostics-"));
    tempRoots.push(root);
    const configHome = join(root, "config");
    const cwd = join(root, "workspace");
    process.env.HOME = join(root, "home");
    process.env.XDG_CONFIG_HOME = configHome;
    process.env.XDG_DATA_HOME = join(root, "data");
    process.env.XDG_CACHE_HOME = join(root, "cache");
    delete process.env.OPENCODE_CONFIG_DIR;
    delete process.env.OPENCODE_DB_PATH;
    mkdirSync(join(configHome, "opencode"), { recursive: true });
    mkdirSync(join(configHome, "eidnara"), { recursive: true });
    mkdirSync(join(cwd, ".eidnara"), { recursive: true });
    return { root, configHome, cwd };
}

describe("collectDiagnostics plugin registration", () => {
    it("recognizes the tuple form of a plugin entry", async () => {
        const { configHome, cwd } = isolatedRoot();
        writeFileSync(
            join(configHome, "opencode", "opencode.json"),
            JSON.stringify({ plugin: [["@eidnara/opencode", { historian: { disable: true } }]] }),
        );
        writeFileSync(
            join(configHome, "opencode", "tui.json"),
            JSON.stringify({ plugin: ["@eidnara/opencode@0.18.0"] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.opencodeConfigHasPlugin).toBe(true);
        expect(report.tuiConfigHasPlugin).toBe(true);
    });

    it("does not treat a tuple naming another package as our plugin", async () => {
        const { configHome, cwd } = isolatedRoot();
        writeFileSync(
            join(configHome, "opencode", "opencode.json"),
            JSON.stringify({ plugin: [["@eidnara/opencode-theme", {}], "other-plugin"] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.opencodeConfigHasPlugin).toBe(false);
    });

    it("records a host-config parse failure instead of reporting a silent absence", async () => {
        const { configHome, cwd } = isolatedRoot();
        writeFileSync(join(configHome, "opencode", "opencode.json"), "{ plugin: [ broken");
        writeFileSync(join(configHome, "opencode", "tui.json"), JSON.stringify({ plugin: [] }));

        const report = await collectDiagnostics(cwd);

        expect(report.opencodeConfigHasPlugin).toBe(false);
        expect(report.opencodeConfigParseError).toBeDefined();
        expect(report.tuiConfigParseError).toBeUndefined();
        expect(renderDiagnosticsMarkdown(report)).toMatch(
            /- opencode config parse error: (?!none)/,
        );
    });

    it("treats a local checkout of this package as a registered plugin", async () => {
        const { root, configHome, cwd } = isolatedRoot();
        const checkout = join(root, "checkout", "packages", "opencode-plugin");
        mkdirSync(checkout, { recursive: true });
        writeFileSync(
            join(checkout, "package.json"),
            JSON.stringify({ name: "@eidnara/opencode", version: "0.0.0" }),
        );
        writeFileSync(
            join(configHome, "opencode", "opencode.json"),
            JSON.stringify({ plugin: [`file://${checkout}`] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.opencodeConfigHasPlugin).toBe(true);
    });

    it("does not treat an unrelated local path as this plugin", async () => {
        const { root, configHome, cwd } = isolatedRoot();
        const other = join(root, "checkout", "eidnara-theme");
        mkdirSync(other, { recursive: true });
        writeFileSync(join(other, "package.json"), JSON.stringify({ name: "eidnara-theme" }));
        writeFileSync(
            join(configHome, "opencode", "opencode.json"),
            JSON.stringify({ plugin: [`file://${other}`] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.opencodeConfigHasPlugin).toBe(false);
    });

    it("reports registration in the project's own opencode config", async () => {
        const { cwd } = isolatedRoot();
        mkdirSync(join(cwd, ".opencode"), { recursive: true });
        writeFileSync(
            join(cwd, ".opencode", "opencode.jsonc"),
            '// project\n{ "plugin": ["@eidnara/opencode"] }',
        );
        writeFileSync(join(cwd, "opencode.json"), "{ broken");

        const report = await collectDiagnostics(cwd);

        expect(report.opencodeConfigHasPlugin).toBe(false);
        expect(report.projectOpencodeConfig.hasPlugin).toBe(true);
        expect(report.projectOpencodeConfig.paths).toEqual([
            join(cwd, ".opencode", "opencode.jsonc"),
            join(cwd, "opencode.json"),
        ]);
        expect(report.projectOpencodeConfig.parseErrors).toHaveLength(1);

        const markdown = renderDiagnosticsMarkdown(report);
        expect(markdown).toContain("- Plugin registered in project opencode config: true (");
        expect(markdown).toMatch(/- project opencode config parse errors: (?!none)/);
    });

    it("lets a project `.jsonc` shadow a sibling `.json`, matching the loader", async () => {
        const { cwd } = isolatedRoot();
        writeFileSync(join(cwd, "opencode.jsonc"), '{ "plugin": [] }');
        writeFileSync(
            join(cwd, "opencode.json"),
            JSON.stringify({ plugin: ["@eidnara/opencode"] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.projectOpencodeConfig.paths).toEqual([join(cwd, "opencode.jsonc")]);
        expect(report.projectOpencodeConfig.hasPlugin).toBe(false);
    });

    it("says so when the project has no opencode config", async () => {
        const { cwd } = isolatedRoot();

        const report = await collectDiagnostics(cwd);

        expect(report.projectOpencodeConfig).toEqual({
            paths: [],
            hasPlugin: false,
            parseErrors: [],
        });
        expect(renderDiagnosticsMarkdown(report)).toContain(
            "- Plugin registered in project opencode config: false (no project opencode config)",
        );
    });
});

describe("collectDiagnostics Eidnara config tiers", () => {
    it("reads the user `.json` fallback and the project tier the loader applies", async () => {
        const { configHome, cwd } = isolatedRoot();
        writeFileSync(
            join(configHome, "eidnara", "eidnara.json"),
            JSON.stringify({ historian: { disable: true } }),
        );
        writeFileSync(
            join(cwd, ".eidnara", "eidnara.jsonc"),
            '// project override\n{ "sidekick": { "disable": true } }',
        );

        const report = await collectDiagnostics(cwd);

        expect(report.eidnaraConfig.path).toBe(join(configHome, "eidnara", "eidnara.json"));
        expect(report.eidnaraConfig.exists).toBe(true);
        expect(report.eidnaraConfig.flags).toEqual({ historian: { disable: true } });
        expect(report.projectConfig.path).toBe(join(cwd, ".eidnara", "eidnara.jsonc"));
        expect(report.projectConfig.exists).toBe(true);
        expect(report.projectConfig.flags).toEqual({ sidekick: { disable: true } });
    });

    it("reports a missing tier at its canonical `.jsonc` path", async () => {
        const { configHome, cwd } = isolatedRoot();

        const report = await collectDiagnostics(cwd);

        expect(report.eidnaraConfig.path).toBe(join(configHome, "eidnara", "eidnara.jsonc"));
        expect(report.eidnaraConfig.exists).toBe(false);
        expect(report.projectConfig.exists).toBe(false);
        expect(report.projectConfig.flags).toEqual({});
    });

    it("captures a project parse error and renders it sanitized", async () => {
        const { configHome, cwd } = isolatedRoot();
        writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), "{ not valid jsonc");

        const report = await collectDiagnostics(cwd);
        expect(report.projectConfig.parseError).toBeDefined();
        expect(report.eidnaraConfig.parseError).toBeUndefined();

        const markdown = renderDiagnosticsMarkdown({
            ...report,
            projectConfig: {
                ...report.projectConfig,
                parseError: `EACCES: permission denied, open '/home/alice/project/.eidnara/eidnara.jsonc' token=abc123`,
            },
        });
        expect(markdown).toContain(
            "- Project config parse error: EACCES: permission denied, open '/home/<USER>/project/.eidnara/eidnara.jsonc' token=<REDACTED:token>",
        );
        expect(markdown).not.toContain("alice");
        expect(markdown).not.toContain("abc123");
        expect(markdown).toContain(
            `- User config: \`${join(configHome, "eidnara", "eidnara.jsonc")}\` (missing)`,
        );
    });
});

describe("collectHistorianDumps", () => {
    it("merges sessions that share a project into one bucket and skips projects without dumps", () => {
        const { root } = isolatedRoot();
        const projectA = join(root, "project-a");
        const projectB = join(root, "project-b");
        mkdirSync(join(projectA, ".eidnara", "context", "historian"), { recursive: true });
        mkdirSync(join(projectB, ".eidnara", "context", "historian"), { recursive: true });
        writeFileSync(join(projectA, ".eidnara", "context", "historian", "dump-1.xml"), "<x/>");
        writeFileSync(join(projectA, ".eidnara", "context", "historian", "dump-2.xml"), "<x/>");

        const dumps = collectHistorianDumps([
            { sessionId: "ses_a1", title: "", directory: projectA, lastActiveAt: "" },
            { sessionId: "ses_a2", title: "", directory: projectA, lastActiveAt: "" },
            { sessionId: "ses_b1", title: "", directory: projectB, lastActiveAt: "" },
        ]);

        expect(dumps.byProject).toHaveLength(1);
        expect(dumps.byProject[0]?.directory).toBe(projectA);
        expect(dumps.byProject[0]?.primarySessionId).toBe("ses_a1");
        expect(dumps.byProject[0]?.sessionIds).toEqual(["ses_a1", "ses_a2"]);
        expect(dumps.byProject[0]?.count).toBe(2);
    });
});

describe("collectDiagnostics recent sessions", () => {
    function seedSessionDb(path: string, directory: string): void {
        mkdirSync(join(path, ".."), { recursive: true });
        const db = new Database(path);
        try {
            db.run(
                "CREATE TABLE session (id TEXT, directory TEXT, title TEXT, time_updated INTEGER, time_archived INTEGER, parent_id TEXT)",
            );
            db.run(
                "INSERT INTO session VALUES ('ses_channel01', ?, 'channel session', 1700000000000, NULL, NULL)",
                [directory],
            );
        } finally {
            db.close();
        }
    }

    it("reads sessions from a channel-specific database when opencode.db is absent", async () => {
        const { root, cwd } = isolatedRoot();
        const dataHome = process.env.XDG_DATA_HOME as string;
        seedSessionDb(join(dataHome, "opencode", "opencode-dev.db"), join(root, "project"));

        const report = await collectDiagnostics(cwd);

        expect(report.recentSessions.map((session) => session.sessionId)).toEqual([
            "ses_channel01",
        ]);
    });

    it("honors OPENCODE_DB_PATH", async () => {
        const { root, cwd } = isolatedRoot();
        const explicit = join(root, "custom", "sessions.db");
        seedSessionDb(explicit, join(root, "project"));
        process.env.OPENCODE_DB_PATH = explicit;

        const report = await collectDiagnostics(cwd);

        expect(report.recentSessions).toHaveLength(1);
    });
});

describe("collectDiagnostics log file", () => {
    it("reports a missing log as absent instead of throwing", async () => {
        const { cwd } = isolatedRoot();

        const report = await collectDiagnostics(cwd);

        expect(report.logFile.exists).toBe(false);
        expect(report.logFile.sizeKb).toBe(0);
    });

    it("reports the size of an existing log", async () => {
        const { cwd } = isolatedRoot();
        const report0 = await collectDiagnostics(cwd);
        mkdirSync(join(report0.logFile.path, ".."), { recursive: true });
        writeFileSync(report0.logFile.path, "x".repeat(2048));

        const report = await collectDiagnostics(cwd);

        expect(report.logFile.exists).toBe(true);
        expect(report.logFile.sizeKb).toBe(2);
    });
});
