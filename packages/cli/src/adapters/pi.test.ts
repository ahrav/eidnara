import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PI_PACKAGE_SOURCE } from "../lib/pi-helpers";
import { PiAdapter } from "./pi";

const originalPiDir = process.env.PI_CODING_AGENT_DIR;
const tempDirs: string[] = [];

afterEach(() => {
    if (originalPiDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
    else process.env.PI_CODING_AGENT_DIR = originalPiDir;
    for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("PiAdapter settings safety", () => {
    it("adds the exact package entry once and reports it present afterwards", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-adapter-"));
        tempDirs.push(root);
        process.env.PI_CODING_AGENT_DIR = root;
        const settingsPath = join(root, "settings.json");
        writeFileSync(settingsPath, JSON.stringify({ packages: ["npm:other"] }));
        const adapter = new PiAdapter();

        const added = await adapter.ensurePluginEntry();
        expect(added).toMatchObject({ ok: true, action: "added", configPath: settingsPath });
        expect(PI_PACKAGE_SOURCE).toBe("npm:@eidnara/pi");
        expect(JSON.parse(readFileSync(settingsPath, "utf-8")).packages).toEqual([
            "npm:other",
            PI_PACKAGE_SOURCE,
        ]);
        expect(adapter.hasPluginEntry()).toBe(true);

        const again = await adapter.ensurePluginEntry();
        expect(again.action).toBe("already_present");
        expect(JSON.parse(readFileSync(settingsPath, "utf-8")).packages).toHaveLength(2);
    });

    it("aborts plugin updates when existing settings are malformed", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-adapter-"));
        tempDirs.push(root);
        process.env.PI_CODING_AGENT_DIR = root;
        const settingsPath = join(root, "settings.json");
        const malformed = `{"packages":[\n`;
        writeFileSync(settingsPath, malformed);

        const result = await new PiAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.message).toContain("Refusing to overwrite unparseable config");
        expect(readFileSync(settingsPath, "utf-8")).toBe(malformed);
    });

    it("keeps existing comments when adding the package entry", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-adapter-"));
        tempDirs.push(root);
        process.env.PI_CODING_AGENT_DIR = root;
        const settingsPath = join(root, "settings.json");
        writeFileSync(
            settingsPath,
            `{\n  // keep me\n  "packages": ["npm:other"] /* trailing */\n}\n`,
        );

        const result = await new PiAdapter().ensurePluginEntry();

        expect(result.action).toBe("added");
        const written = readFileSync(settingsPath, "utf-8");
        expect(written).toContain("// keep me");
        expect(written).toContain("/* trailing */");
        expect(written).toContain(PI_PACKAGE_SOURCE);
    });

    it("reports the plugin absent instead of throwing when packages is not an array", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-adapter-"));
        tempDirs.push(root);
        process.env.PI_CODING_AGENT_DIR = root;
        writeFileSync(join(root, "settings.json"), JSON.stringify({ packages: "npm:other" }));

        expect(new PiAdapter().hasPluginEntry()).toBe(false);
    });
});
