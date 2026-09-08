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
