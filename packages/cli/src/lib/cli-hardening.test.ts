import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { pathToFileURL } from "node:url";
import { isDevPathPluginEntry } from "../adapters/opencode";
import { resolveAdaptersForCommand } from "./harness-select";
import { detectConfigPaths, envFirstHomeDir, getOpenCodeConfigDir } from "./paths";

const roots: string[] = [];
const originalOpenCodeConfigDir = process.env.OPENCODE_CONFIG_DIR;
const originalXdgConfigHome = process.env.XDG_CONFIG_HOME;

afterEach(() => {
    if (originalOpenCodeConfigDir === undefined) delete process.env.OPENCODE_CONFIG_DIR;
    else process.env.OPENCODE_CONFIG_DIR = originalOpenCodeConfigDir;
    if (originalXdgConfigHome === undefined) delete process.env.XDG_CONFIG_HOME;
    else process.env.XDG_CONFIG_HOME = originalXdgConfigHome;
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

    it.if(process.platform !== "win32")("ignores a relative HOME", () => {
        const originalHome = process.env.HOME;
        try {
            process.env.HOME = "workspace-home";
            const home = envFirstHomeDir();
            expect(isAbsolute(home)).toBe(true);
            expect(home).not.toBe("workspace-home");
        } finally {
            if (originalHome === undefined) delete process.env.HOME;
            else process.env.HOME = originalHome;
        }
    });

    it.if(process.platform !== "win32")("ignores a relative XDG_CONFIG_HOME", () => {
        const originalXdg = process.env.XDG_CONFIG_HOME;
        delete process.env.OPENCODE_CONFIG_DIR;
        process.env.XDG_CONFIG_HOME = ".config";
        try {
            expect(getOpenCodeConfigDir()).toBe(join(homedir(), ".config", "opencode"));
            process.env.XDG_CONFIG_HOME = "/virt/xdg";
            expect(getOpenCodeConfigDir()).toBe("/virt/xdg/opencode");
        } finally {
            if (originalXdg === undefined) delete process.env.XDG_CONFIG_HOME;
            else process.env.XDG_CONFIG_HOME = originalXdg;
        }
    });

    it("selects opencode.jsonc for a fresh configuration", () => {
        const root = tempRoot();
        process.env.OPENCODE_CONFIG_DIR = root;
        const paths = detectConfigPaths();
        expect(paths.opencodeConfig).toBe(join(root, "opencode.jsonc"));
        expect(paths.opencodeConfigFormat).toBe("none");
    });

    it("targets an existing eidnara.json instead of shadowing it with a new eidnara.jsonc", () => {
        const root = tempRoot();
        process.env.OPENCODE_CONFIG_DIR = join(root, "opencode");
        process.env.XDG_CONFIG_HOME = root;
        const eidnaraDir = join(root, "eidnara");
        mkdirSync(eidnaraDir, { recursive: true });

        expect(detectConfigPaths().eidnaraConfig).toBe(join(eidnaraDir, "eidnara.jsonc"));

        writeFileSync(join(eidnaraDir, "eidnara.json"), `{"compaction":{"enabled":false}}`);
        expect(detectConfigPaths().eidnaraConfig).toBe(join(eidnaraDir, "eidnara.json"));

        writeFileSync(join(eidnaraDir, "eidnara.jsonc"), "{}");
        expect(detectConfigPaths().eidnaraConfig).toBe(join(eidnaraDir, "eidnara.jsonc"));
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
