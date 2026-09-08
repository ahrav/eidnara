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
});
