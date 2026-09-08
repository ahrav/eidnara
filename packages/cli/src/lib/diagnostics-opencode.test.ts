import { Database } from "bun:sqlite";
import { afterEach, describe, expect, it, setDefaultTimeout, spyOn } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import os, { tmpdir } from "node:os";
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
    "OPENCODE_DISABLE_PROJECT_CONFIG",
    "EIDNARA_LOG_PATH",
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
    process.env.EIDNARA_LOG_PATH = join(root, "log", "eidnara.log");
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

    it("reports a registration that only a custom or inline layer provides", async () => {
        const { root, cwd } = isolatedRoot();
        const savedConfig = process.env.OPENCODE_CONFIG;
        const savedContent = process.env.OPENCODE_CONFIG_CONTENT;
        try {
            const custom = join(root, "custom.json");
            writeFileSync(custom, JSON.stringify({ plugin: ["@eidnara/opencode"] }));
            process.env.OPENCODE_CONFIG = custom;
            delete process.env.OPENCODE_CONFIG_CONTENT;

            const report = await collectDiagnostics(cwd);

            expect(report.opencodeConfigHasPlugin).toBe(false);
            expect(report.projectOpencodeConfig.hasPlugin).toBe(false);
            expect(report.pluginRegisteredInLoadedLayers).toBe(true);
            expect(renderDiagnosticsMarkdown(report)).toContain(
                "- Plugin registered in any loaded OpenCode config layer: true",
            );

            delete process.env.OPENCODE_CONFIG;
            process.env.OPENCODE_CONFIG_CONTENT = JSON.stringify({ plugin: ["@eidnara/opencode"] });
            expect((await collectDiagnostics(cwd)).pluginRegisteredInLoadedLayers).toBe(true);
        } finally {
            if (savedConfig === undefined) delete process.env.OPENCODE_CONFIG;
            else process.env.OPENCODE_CONFIG = savedConfig;
            if (savedContent === undefined) delete process.env.OPENCODE_CONFIG_CONTENT;
            else process.env.OPENCODE_CONFIG_CONTENT = savedContent;
        }
    });

    it("reads a registration from either user-level sibling, since the host merges both", async () => {
        const { configHome, cwd } = isolatedRoot();
        writeFileSync(join(configHome, "opencode", "opencode.jsonc"), '{ "plugin": [] }');
        writeFileSync(
            join(configHome, "opencode", "opencode.json"),
            JSON.stringify({ plugin: ["@eidnara/opencode"] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.configPaths.opencodeConfig).toBe(
            join(configHome, "opencode", "opencode.jsonc"),
        );
        expect(report.opencodeConfigHasPlugin).toBe(true);
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

    it("reads a registration from a `.json` sibling of a project `.jsonc`, matching the host", async () => {
        const { cwd } = isolatedRoot();
        writeFileSync(join(cwd, "opencode.jsonc"), '{ "plugin": [] }');
        writeFileSync(
            join(cwd, "opencode.json"),
            JSON.stringify({ plugin: ["@eidnara/opencode"] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.projectOpencodeConfig.paths).toEqual([
            join(cwd, "opencode.jsonc"),
            join(cwd, "opencode.json"),
        ]);
        expect(report.projectOpencodeConfig.hasPlugin).toBe(true);
    });

    it("reports no project registration when OpenCode disables project config", async () => {
        const { cwd } = isolatedRoot();
        writeFileSync(
            join(cwd, "opencode.json"),
            JSON.stringify({ plugin: ["@eidnara/opencode"] }),
        );
        process.env.OPENCODE_DISABLE_PROJECT_CONFIG = "true";

        const report = await collectDiagnostics(cwd);

        expect(report.projectOpencodeConfig.paths).toEqual([]);
        expect(report.projectOpencodeConfig.hasPlugin).toBe(false);
    });

    it.if(process.platform !== "win32")(
        "reports a FIFO at a config path as a parse error instead of blocking",
        async () => {
            const { cwd } = isolatedRoot();
            execFileSync("mkfifo", [join(cwd, "opencode.json")]);

            const report = await collectDiagnostics(cwd);

            expect(report.projectOpencodeConfig.paths).toEqual([join(cwd, "opencode.json")]);
            expect(report.projectOpencodeConfig.hasPlugin).toBe(false);
            expect(report.projectOpencodeConfig.parseErrors.join("\n")).toContain(
                "not a regular file",
            );
        },
    );

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

    it("falls back to the next database when the newest candidate is not a session database", async () => {
        const { root, cwd } = isolatedRoot();
        const dataHome = process.env.XDG_DATA_HOME as string;
        seedSessionDb(join(dataHome, "opencode", "opencode.db"), join(root, "project"));
        // A newer file that matches `opencode*.db` but holds no session table.
        const stray = join(dataHome, "opencode", "opencode-backup.db");
        writeFileSync(stray, "not a database");
        const later = Date.now() / 1000 + 60;
        utimesSync(stray, later, later);

        const report = await collectDiagnostics(cwd);

        expect(report.recentSessions.map((session) => session.sessionId)).toEqual([
            "ses_channel01",
        ]);
    });

    it("ignores a relative XDG_DATA_HOME instead of reading a database under the working directory", async () => {
        const { root, cwd } = isolatedRoot();
        // A checkout could plant `data/opencode/opencode.db` and point a relative XDG root at it.
        seedSessionDb(join(cwd, "data", "opencode", "opencode.db"), join(root, "project"));
        process.env.XDG_DATA_HOME = "data";
        const originalCwd = process.cwd();
        process.chdir(cwd);
        try {
            const report = await collectDiagnostics(cwd);
            expect(report.recentSessions).toEqual([]);
        } finally {
            process.chdir(originalCwd);
        }
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
        const { root, cwd } = isolatedRoot();
        mkdirSync(join(root, "log"), { recursive: true });
        writeFileSync(join(root, "log", "eidnara.log"), "x".repeat(2048));

        const report = await collectDiagnostics(cwd);

        expect(report.logFile.path).toBe(join(root, "log", "eidnara.log"));
        expect(report.logFile.exists).toBe(true);
        expect(report.logFile.sizeKb).toBe(2);
    });
});

describe("collectHistorianDumps entry failures", () => {
    it("keeps the valid dumps when one directory entry cannot be statted", () => {
        const { root } = isolatedRoot();
        const project = join(root, "project-a");
        const historianDir = join(project, ".eidnara", "context", "historian");
        mkdirSync(historianDir, { recursive: true });
        writeFileSync(join(historianDir, "dump-1.xml"), "<x/>");
        // A dangling symlink is listed by readdir but fails stat, like a file removed mid-walk.
        symlinkSync(join(root, "gone.xml"), join(historianDir, "dump-2.xml"));

        const dumps = collectHistorianDumps([
            { sessionId: "ses_a1", title: "", directory: project, lastActiveAt: "" },
        ]);

        expect(dumps.byProject).toHaveLength(1);
        expect(dumps.byProject[0]?.count).toBe(1);
        expect(dumps.byProject[0]?.recent.map((dump) => dump.name)).toEqual(["dump-1.xml"]);
    });
});

describe("collectDiagnostics relative plugin entries", () => {
    it("resolves a relative local checkout against the inspected project, not process.cwd()", async () => {
        const { cwd } = isolatedRoot();
        const checkout = join(cwd, "vendor", "opencode-plugin");
        mkdirSync(checkout, { recursive: true });
        writeFileSync(
            join(checkout, "package.json"),
            JSON.stringify({ name: "@eidnara/opencode" }),
        );
        writeFileSync(
            join(cwd, "opencode.json"),
            JSON.stringify({ plugin: ["./vendor/opencode-plugin"] }),
        );

        const report = await collectDiagnostics(cwd);

        expect(report.projectOpencodeConfig.hasPlugin).toBe(true);
    });
});

describe("renderDiagnosticsMarkdown path sanitization", () => {
    it("redacts secret material embedded in a reported path", async () => {
        const { root, cwd } = isolatedRoot();
        process.env.EIDNARA_LOG_PATH = join(root, "token=abc123", "eidnara.log");

        const report = await collectDiagnostics(cwd);
        const markdown = renderDiagnosticsMarkdown(report);

        expect(markdown).toContain("- Path: ");
        expect(markdown).not.toContain("abc123");
        expect(markdown).toContain("token=<REDACTED:token>");
    });
});

describe("collectDiagnostics without any home directory", () => {
    it("produces a partial report when user-level paths cannot be resolved", async () => {
        const { root, cwd } = isolatedRoot();
        delete process.env.HOME;
        delete process.env.XDG_CONFIG_HOME;
        delete process.env.XDG_DATA_HOME;
        const spy = spyOn(os, "homedir").mockImplementation(() => {
            throw Object.assign(new Error("uv_os_homedir returned ENOENT"), {
                code: "ERR_SYSTEM_ERROR",
            });
        });
        try {
            const report = await collectDiagnostics(cwd);
            expect(report.configPathsError).toContain("uv_os_homedir");
            expect(report.configPaths.opencodeConfigFormat).toBe("none");
            expect(report.eidnaraConfig.exists).toBe(false);
            expect(report.projectConfig.path).toBe(join(cwd, ".eidnara", "eidnara.jsonc"));
            expect(report.projectDirectory).toBe(cwd);
            expect(renderDiagnosticsMarkdown(report)).toContain("- User-level paths unavailable: ");
            expect(root).toBeTruthy();
        } finally {
            spy.mockRestore();
        }
    });
});
