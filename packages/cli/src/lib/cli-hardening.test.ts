import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { isDevPathPluginEntry } from "../adapters/opencode";
import { resolveAdaptersForCommand } from "./harness-select";
import { detectConfigPaths } from "./paths";

const roots: string[] = [];
const originalOpenCodeConfigDir = process.env.OPENCODE_CONFIG_DIR;

afterEach(() => {
    if (originalOpenCodeConfigDir === undefined) delete process.env.OPENCODE_CONFIG_DIR;
    else process.env.OPENCODE_CONFIG_DIR = originalOpenCodeConfigDir;
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function tempRoot(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-cli-hardening-"));
    roots.push(root);
    return root;
}

describe("CLI hardening helpers", () => {
    it("rejects invalid harness overrides instead of treating them as absent", async () => {
        await expect(
            resolveAdaptersForCommand(["--harness", "opencdoe"], {
                allowMulti: false,
                verb: "setup",
            }),
        ).rejects.toThrow("Invalid --harness value: opencdoe");
    });

    it("selects opencode.jsonc for a fresh configuration", () => {
        const root = tempRoot();
        process.env.OPENCODE_CONFIG_DIR = root;
        const paths = detectConfigPaths();
        expect(paths.opencodeConfig).toBe(join(root, "opencode.jsonc"));
        expect(paths.opencodeConfigFormat).toBe("none");
    });

    it("reports an existing eidnara.json as the Eidnara config instead of the absent .jsonc", () => {
        const root = tempRoot();
        process.env.OPENCODE_CONFIG_DIR = root;
        const originalConfigHome = process.env.XDG_CONFIG_HOME;
        process.env.XDG_CONFIG_HOME = join(root, "xdg");
        try {
            const configDir = join(root, "xdg", "eidnara");
            mkdirSync(configDir, { recursive: true });
            expect(detectConfigPaths().eidnaraConfig).toBe(join(configDir, "eidnara.jsonc"));
            writeFileSync(join(configDir, "eidnara.json"), "{}");
            expect(detectConfigPaths().eidnaraConfig).toBe(join(configDir, "eidnara.json"));
            writeFileSync(join(configDir, "eidnara.jsonc"), "{}");
            expect(detectConfigPaths().eidnaraConfig).toBe(join(configDir, "eidnara.jsonc"));
        } finally {
            if (originalConfigHome === undefined) delete process.env.XDG_CONFIG_HOME;
            else process.env.XDG_CONFIG_HOME = originalConfigHome;
        }
    });

    it("accepts only local development paths with the exact package name", () => {
        const root = tempRoot();
        const plugin = join(root, "plugin");
        const theme = join(root, "eidnara-theme");
        mkdirSync(plugin, { recursive: true });
        mkdirSync(theme, { recursive: true });
        writeFileSync(join(plugin, "package.json"), JSON.stringify({ name: "@eidnara/opencode" }));
        writeFileSync(join(theme, "package.json"), JSON.stringify({ name: "eidnara-theme" }));

        expect(isDevPathPluginEntry(pathToFileURL(plugin).href)).toBe(true);
        expect(isDevPathPluginEntry(pathToFileURL(theme).href)).toBe(false);
    });
});
