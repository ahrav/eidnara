import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { OpenCodeAdapter } from "./opencode";

const originalConfigDir = process.env.OPENCODE_CONFIG_DIR;
const roots: string[] = [];

afterEach(() => {
    if (originalConfigDir === undefined) delete process.env.OPENCODE_CONFIG_DIR;
    else process.env.OPENCODE_CONFIG_DIR = originalConfigDir;
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function configRoot(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-opencode-adapter-"));
    roots.push(root);
    process.env.OPENCODE_CONFIG_DIR = root;
    return root;
}

describe("OpenCodeAdapter config safety", () => {
    it("keeps existing comments when adding the plugin entry", async () => {
        const root = configRoot();
        const configPath = join(root, "opencode.jsonc");
        writeFileSync(configPath, `{\n    // keep me\n    "plugin": ["other"] /* trailing */\n}\n`);

        const result = await new OpenCodeAdapter().ensurePluginEntry();

        expect(result).toMatchObject({ ok: true, action: "added", configPath });
        const written = readFileSync(configPath, "utf-8");
        expect(written).toContain("// keep me");
        expect(written).toContain("/* trailing */");
        expect(written).toContain('"@eidnara/opencode"');
    });

    it("refuses an array document root instead of reporting a phantom add", async () => {
        const root = configRoot();
        const configPath = join(root, "opencode.json");
        const original = `["not", "an", "object"]\n`;
        writeFileSync(configPath, original);

        const result = await new OpenCodeAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.action).toBe("error");
        expect(result.message).toContain("expected a JSON object at the document root");
        expect(readFileSync(configPath, "utf-8")).toBe(original);
    });

    it("refuses prototype-pollution keys before mutating the config", async () => {
        const root = configRoot();
        const configPath = join(root, "opencode.json");
        const original = `{"__proto__": {"plugin": ["@eidnara/opencode"]}}\n`;
        writeFileSync(configPath, original);
        const adapter = new OpenCodeAdapter();

        const result = await adapter.ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.message).toContain("prototype-pollution");
        expect(readFileSync(configPath, "utf-8")).toBe(original);
        expect(adapter.hasPluginEntry()).toBe(false);
    });

    it("reports a verified dev-path entry as present", async () => {
        const root = configRoot();
        const plugin = join(root, "plugin");
        mkdirSync(plugin, { recursive: true });
        writeFileSync(join(plugin, "package.json"), JSON.stringify({ name: "@eidnara/opencode" }));
        const configPath = join(root, "opencode.json");
        writeFileSync(configPath, JSON.stringify({ plugin: [pathToFileURL(plugin).href] }));
        const adapter = new OpenCodeAdapter();

        expect(adapter.hasPluginEntry()).toBe(true);
        const result = await adapter.ensurePluginEntry();
        expect(result.action).toBe("already_present");
    });
});
