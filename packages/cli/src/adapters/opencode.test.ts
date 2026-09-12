import { afterEach, describe, expect, it } from "bun:test";
import {
    existsSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { isLocalPathPluginEntry, OpenCodeAdapter } from "./opencode";

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

    it.if(process.platform !== "win32")(
        "writes through a dangling opencode.json symlink instead of creating opencode.jsonc",
        async () => {
            const root = configRoot();
            const target = join(root, "dotfiles", "opencode.json");
            const link = join(root, "opencode.json");
            symlinkSync(target, link);
            expect(existsSync(target)).toBe(false);

            const result = await new OpenCodeAdapter().ensurePluginEntry();

            expect(result).toMatchObject({ ok: true, action: "added", configPath: link });
            expect(lstatSync(link).isSymbolicLink()).toBe(true);
            expect(JSON.parse(readFileSync(target, "utf-8")).plugin).toEqual(["@eidnara/opencode"]);
            expect(existsSync(join(root, "opencode.jsonc"))).toBe(false);
        },
    );

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

    it.if(process.platform !== "win32")(
        "reports a home-relative dev path as present when its manifest names the plugin",
        async () => {
            const root = configRoot();
            const home = join(root, "home");
            const plugin = join(home, "src", "opencode-plugin");
            mkdirSync(plugin, { recursive: true });
            writeFileSync(
                join(plugin, "package.json"),
                JSON.stringify({ name: "@eidnara/opencode" }),
            );
            writeFileSync(
                join(root, "opencode.json"),
                JSON.stringify({ plugin: ["~/src/opencode-plugin"] }),
            );
            const originalHome = process.env.HOME;
            process.env.HOME = home;
            try {
                const adapter = new OpenCodeAdapter();
                expect(adapter.hasPluginEntry()).toBe(true);
                expect((await adapter.ensurePluginEntry()).action).toBe("already_present");
            } finally {
                if (originalHome === undefined) delete process.env.HOME;
                else process.env.HOME = originalHome;
            }
        },
    );

    it("refuses to replace a non-array plugin value", async () => {
        const root = configRoot();
        const configPath = join(root, "opencode.json");
        const before = JSON.stringify({ plugin: "custom-loader" });
        writeFileSync(configPath, before);

        const result = await new OpenCodeAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.action).toBe("error");
        expect(result.message).toContain("not an array");
        expect(readFileSync(configPath, "utf-8")).toBe(before);
    });

    it("leaves a version-pinned plugin entry as written", async () => {
        const root = configRoot();
        const configPath = join(root, "opencode.json");
        const before = JSON.stringify({ plugin: ["@eidnara/opencode@0.1.0"] });
        writeFileSync(configPath, before);
        const adapter = new OpenCodeAdapter();

        expect(adapter.hasPluginEntry()).toBe(true);
        const result = await adapter.ensurePluginEntry();
        expect(result.action).toBe("already_present");
        expect(readFileSync(configPath, "utf-8")).toBe(before);
    });
});

describe("isLocalPathPluginEntry", () => {
    it("recognizes file URLs, absolute, relative, and home-relative paths with either separator", () => {
        expect(isLocalPathPluginEntry("file:///opt/eidnara/opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("FILE:///opt/eidnara/opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("/opt/eidnara/opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("./packages/opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("../opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry(".\\packages\\opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("..\\opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("~/src/eidnara/packages/opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry("~\\src\\opencode-plugin")).toBe(true);
        expect(isLocalPathPluginEntry([".\\packages\\opencode-plugin", {}])).toBe(true);
    });

    it("does not treat package names or dot- or tilde-prefixed names as paths", () => {
        expect(isLocalPathPluginEntry("@eidnara/opencode")).toBe(false);
        expect(isLocalPathPluginEntry("@eidnara/opencode@0.1.0")).toBe(false);
        expect(isLocalPathPluginEntry(".eidnara")).toBe(false);
        expect(isLocalPathPluginEntry("..eidnara")).toBe(false);
        expect(isLocalPathPluginEntry("~eidnara")).toBe(false);
        expect(isLocalPathPluginEntry(42)).toBe(false);
    });
});
