import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collectDiagnostics } from "./diagnostics-opencode";

const roots: string[] = [];
const original = {
    HOME: process.env.HOME,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
    OPENCODE_CONFIG_DIR: process.env.OPENCODE_CONFIG_DIR,
};
const originalCwd = process.cwd();

afterEach(() => {
    process.chdir(originalCwd);
    for (const [key, value] of Object.entries(original)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function isolate(): { root: string; configDir: string; project: string } {
    const root = mkdtempSync(join(tmpdir(), "eidnara-oc-diagnostics-"));
    roots.push(root);
    const configDir = join(root, "opencode");
    const project = join(root, "project");
    mkdirSync(configDir, { recursive: true });
    mkdirSync(join(project, ".eidnara"), { recursive: true });
    mkdirSync(join(root, "xdg", "eidnara"), { recursive: true });
    process.env.HOME = root;
    process.env.XDG_CONFIG_HOME = join(root, "xdg");
    process.env.XDG_DATA_HOME = join(root, "data");
    process.env.OPENCODE_CONFIG_DIR = configDir;
    process.chdir(project);
    return { root, configDir, project };
}

describe("collectDiagnostics", () => {
    it("recognizes the tuple plugin entry form the doctor accepts", async () => {
        const { configDir } = isolate();
        writeFileSync(
            join(configDir, "opencode.jsonc"),
            JSON.stringify({ plugin: [["@eidnara/opencode", { enabled: true }]] }),
        );
        writeFileSync(
            join(configDir, "tui.jsonc"),
            JSON.stringify({ plugin: ["@eidnara/opencode@1.0.0"] }),
        );

        const report = await collectDiagnostics();

        expect(report.opencodeConfigHasPlugin).toBe(true);
        expect(report.tuiConfigHasPlugin).toBe(true);
    });

    it("collects the project-tier config from an explicit directory instead of the current one", async () => {
        const { root, project } = isolate();
        writeFileSync(
            join(project, ".eidnara", "eidnara.json"),
            JSON.stringify({ historian: { model: "anthropic/current" } }),
        );
        const other = join(root, "other-project");
        mkdirSync(join(other, ".eidnara"), { recursive: true });
        writeFileSync(
            join(other, ".eidnara", "eidnara.json"),
            JSON.stringify({ historian: { model: "anthropic/other" } }),
        );

        const report = await collectDiagnostics(other);

        expect(report.projectConfig.path).toBe(join(other, ".eidnara", "eidnara.json"));
        expect(report.projectConfig.flags).toEqual({ historian: { model: "anthropic/other" } });
    });

    it("collects the project-tier config from the current directory", async () => {
        const { project } = isolate();
        writeFileSync(
            join(project, ".eidnara", "eidnara.json"),
            JSON.stringify({ historian: { model: "anthropic/claude", api_key: "secret" } }),
        );

        const report = await collectDiagnostics();

        expect(report.projectConfig.path).toBe(join(project, ".eidnara", "eidnara.json"));
        expect(report.projectConfig.exists).toBe(true);
        expect(report.projectConfig.flags).toEqual({
            historian: { model: "anthropic/claude", api_key: "<REDACTED:api_key>" },
        });
        expect(report.eidnaraConfig.exists).toBe(false);
    });

    it("reports no conflicts when Eidnara is disabled, even with DCP and native compaction on", async () => {
        const { root, configDir } = isolate();
        writeFileSync(
            join(configDir, "opencode.jsonc"),
            JSON.stringify({
                plugin: ["@eidnara/opencode", "@tarquinen/opencode-dcp"],
                compaction: { auto: true },
            }),
        );
        writeFileSync(
            join(root, "xdg", "eidnara", "eidnara.jsonc"),
            JSON.stringify({ enabled: false }),
        );

        const report = await collectDiagnostics();

        expect(report.conflicts.eidnaraEnabled).toBe(false);
        expect(report.conflicts.compactionEnabled).toBe(false);
        expect(report.conflicts.hasConflict).toBe(false);
        expect(report.conflicts.reasons).toEqual([]);
    });

    it("still reports the DCP conflict when Eidnara is enabled", async () => {
        const { configDir } = isolate();
        writeFileSync(
            join(configDir, "opencode.jsonc"),
            JSON.stringify({
                plugin: ["@eidnara/opencode", "@tarquinen/opencode-dcp"],
                compaction: { auto: false, prune: false },
            }),
        );

        const report = await collectDiagnostics();

        expect(report.conflicts.eidnaraEnabled).toBe(true);
        expect(report.conflicts.hasConflict).toBe(true);
        expect(report.conflicts.reasons.some((reason) => /dcp/i.test(reason))).toBe(true);
    });

    it("reports session discovery as unavailable when the database cannot be read", async () => {
        const { root } = isolate();
        mkdirSync(join(root, "data", "opencode"), { recursive: true });
        writeFileSync(join(root, "data", "opencode", "opencode.db"), "not a sqlite database");

        const report = await collectDiagnostics();

        expect(report.recentSessions).toEqual([]);
        expect(report.sessionDiscovery).toBe("unavailable");
    });

    it("reports session discovery as unavailable when no database exists", async () => {
        isolate();

        const report = await collectDiagnostics();

        // The append-only log can outlive the database, so its records stay
        // unattributable and the issue flow must ask before bundling them.
        expect(report.recentSessions).toEqual([]);
        expect(report.sessionDiscovery).toBe("unavailable");
    });
});
